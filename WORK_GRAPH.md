# Fleet — Top-Down Work Graph & Phase-Gated Build

> Companion to `PLAN.md` (decisions + vertical slices S0-S26) and `docs/ENGINEERING_SPEC.md` (spec).
> This document re-expresses the build **top-down** — goal → phases → nodes — as an **explicit
> dependency DAG** with **phase gates whose exit criterion is heavy unit testing**, designed to be
> executed by a **dynamic workflow** whose parallelism **widens after each gate** and converges at
> the v1 Definition of Done.
>
> **Shape:** width `1 → 2 →│G0│→ 3 → 5 →│G1│→ ~10 →│G2/G3│→ 2 → 1`. Narrow while the spine is
> unproven; widest once the protocol + reporter contracts are gated-green; convergent at acceptance.

---

## 1. Top-down decomposition (goal → layers)

```
GOAL  Fleet v1 (DoD §21): live agent state across editors → one auto-resolving inbox, mac+linux
 │
 ├─ L0  CONTRACTS & SPINE      the protocol + Hub everything else consumes        ──┐ everything
 ├─ L1  REPORTER FRAMEWORK     registration, durable identity, persistence          │ depends
 ├─ L2  DETECTION ADAPTERS     Codex hooks, Claude hooks/inference, extension shim   │ downward
 ├─ L3  FACES / UI             Tauri inbox, notifications, focus, palette, mute      │
 └─ L4  INTEGRATION & DoD      multi-editor, cross-OS, §21 acceptance              ──┘
```

A **layer** is a horizontal band of the DAG. A **phase** is the scheduling unit between two gates.
Parallelism increases because each gate freezes a contract (protocol → reporter API → adapter
state-model → face reducer) that unblocks a wider set of mutually-independent nodes.

---

## 2. The explicit work graph (nodes + edges)

Every node is one buildable+unit-testable unit. `deps` are hard predecessors (must be **green**,
i.e. past their gate where a gate intervenes). Gate nodes (`◆`) are **barriers**: heavy unit
testing that must pass before any dependent starts. Slice refs trace to `PLAN.md`.

| Node | Title | Slice | Phase | Depends on |
|---|---|---|---|---|
| `REPO` | Monorepo scaffold + CI matrix (mac+linux) | S0 | P0 | — |
| `PROTO` | `fleet-protocol` crate: types, JSON Schema, TS gen | S1 | P0 | `REPO` |
| `HUB` | Hub spine: WS + unix(`cfg(unix)`), lockfile, subscribe, merge | S2 | P0 | `PROTO` |
| `CLI` | `fleet ls` face (also the test oracle) | S3 | P0 | `HUB` |
| `FAKE` | Fake reporter / fixture generator | S4 | P0 | `HUB` |
| `◆G0` | **GATE: spine** | — | G0 | `CLI`, `FAKE` |
| `REPCORE` | Reporter: outbound register, heartbeat, buffer/reconnect | S5 | P1 | `◆G0` |
| `IDENTITY` | Durable identity: seq + ordered replay + atomic GC | S6 | P1 | `REPCORE` |
| `PERSIST` | SQLite event log, restart-restore, reap | S7 | P1 | `◆G0` |
| `EXTSKEL` | VS Code extension skeleton (Open-VSX, `^1.93`) | S8 | P1 | `REPCORE` |
| `UISHELL` | Tauri read-only inbox mirroring `fleet ls` | S19 | P1 | `◆G0` |
| `◆G1` | **GATE: reporter framework** | — | G1 | `IDENTITY`, `PERSIST` |
| `ENVINJ` | `EnvironmentVariableCollection` injection | S9 | P2 | `EXTSKEL` |
| `SHIM` | PATH-shim `claude`/`codex` wrappers (B′) | S10 | P2 | `ENVINJ` |
| `CODEX` | Codex hooks detect + approval + auto-resolve | S11–13 | P2 | `SHIM`, `◆G1` |
| `INIT` | `fleet init`/`uninit` config fallback writer | S14 | P2 | `◆G1` |
| `CLHOOK` | Claude working/idle/done via hooks | S15 | P2 | `INIT` |
| `CLINFER` | Claude inferred waiting + JSONL drift-guard | S16 | P2 | `CLHOOK` |
| `CLUSETERM` | Claude high-confidence in shim terminal | S17 | P2 | `SHIM`, `CLHOOK` |
| `READSTREAM` | Shell-integration read-stream OSC recovery | S18 | P2 | `EXTSKEL` |
| `UISORT` | Urgency + age + sort | S20 | P3 | `UISHELL` |
| `UINOTIFY` | OS notifications + auto-resolve + sound tiers | S21 | P3 | `UISHELL` |
| `UICONF` | Confidence surfacing (hollow/solid) | S22 | P3 | `UISHELL` |
| `FOCUS` | Per-OS focus (mac / X11 / Wayland-fallback) | S23 | P3 | `UISHELL` |
| `PALETTE` | Fuzzy palette + cycle-unread | S24 | P3 | `UISHELL` |
| `MUTE` | Mute / solo Hub commands | S25 | P3 | `UISHELL`, `HUB` |
| `◆G2` | **GATE: detection adapters** | — | G2 | `CODEX`,`CLINFER`,`CLUSETERM`,`READSTREAM` |
| `◆G3` | **GATE: faces/UI** | — | G3 | `UISORT`,`UINOTIFY`,`UICONF`,`FOCUS`,`PALETTE`,`MUTE` |
| `EDITORS` | Multi-editor descriptor table + launch/focus | S26 | P4 | `FOCUS`, `◆G2` |
| `E2E` | §21 DoD scenarios as integration tests | — | P4 | `◆G2`, `◆G3`, `EDITORS` |
| `◆G4` | **GATE: v1 Definition of Done** | — | G4 | `E2E` |

### DAG (data flow left→right; vertical stacks run in parallel)

```
                         ┌────────────────── P2 DETECTION ──────────────────┐
                         │ EXTSKEL→ENVINJ→SHIM→CODEX ───────────────┐       │
 REPO→PROTO→HUB→┬─CLI─┐  │           └→READSTREAM                    │       │
                └─FAKE┤  │ INIT→CLHOOK→┬─CLINFER ───────────────────┤       │
                      │  │             └─CLUSETERM (also needs SHIM)─┘       │
                      ▼  │                                                   ▼
                    ◆G0 ─┼─→ REPCORE→IDENTITY ─┐                           ◆G2 ─┐
                      │  │   PERSIST ───────────┼─→ ◆G1 ──────────────────────┐  ├→EDITORS→E2E→◆G4
                      │  │   EXTSKEL            │   (P2 nodes' G1 dep)         │  │
                      └──┴─→ UISHELL ─┬─ UISORT ─┬─ UINOTIFY ─┬─ UICONF ──┐   │  │
                         (P3 FACES)   ├─ FOCUS ──┴─ PALETTE ──┴─ MUTE ────┼──→◆G3┘
                                      └──────────────────────────────────┘
```

P2 (detection) and P3 (faces) are **independent bands** that run concurrently after `◆G0`/`◆G1`,
which is where peak width (~10 concurrent nodes) occurs.

---

## 3. Phase gates — exit criterion is HEAVY UNIT TESTING

A gate is a **hard barrier**: no dependent node starts until the gate is green. Each gate runs the
**full unit suite for everything in its phase plus all prior phases** (no regressions), enforces a
coverage floor, and adds the property/fuzz tests that make the frozen contract trustworthy. A gate
that fails dispatches **fixer agents and loops to green (≤3 rounds), else HALTS the build**.

| Gate | Freezes | Heavy-unit-test exit criteria | Cov floor |
|---|---|---|---|
| `◆G0` | **Protocol + Hub** | proptest round-trip on **every** `Session`/`AgentRun`/event/command variant; JSON-Schema conformance both directions; Hub merge-engine property test (`rollup = most-urgent across runs`); transport tests (WS **and** unix); lockfile/single-instance; CLI render-snapshot; **two-face consistency** driven by `FAKE`; zero `clippy` warnings | ≥85% proto+hub |
| `◆G1` | **Reporter API** | registration/heartbeat/reconnect; **identity proptests for the 3 S6 invariants** (idempotent `(durable_id,seq)`, ordered-replay-by-seq, **atomic** entry+buffer expiry-GC); duplicate + out-of-order + reconnect-vs-fresh fuzz → **no ghost**; persistence restart-restore + **crash-mid-write** recovery; buffer/replay across simulated disconnect | ≥85% reporter+identity+persist |
| `◆G2` | **Detection state-model** | each adapter vs **recorded fixtures** (Codex hook JSON, Claude hook JSON, transcript JSONL, OSC byte streams); state-machine property test (**no illegal transition**); **confidence-honesty invariant** (never `high` without an authoritative signal); inference debounce timing; **schema-drift fuzz** (malformed JSONL → degrades, never panics or overstates) | ≥80% adapters |
| `◆G3` | **Face reducer** | UI reducer determinism (snapshot+delta→view); exhaustive sort ordering `(unread,urgency,age)`; notification **urgency→sound-name** mapping table; confidence render; **per-OS focus with mocked OS calls + focus-confirmation telemetry**; palette fuzzy-match; mute/solo command round-trips | ≥80% face logic |
| `◆G4` | **v1 DoD** | §21 items 1–11 automated as **integration** scenarios on mac+linux (≥3 agents/≥2 editors; Codex `high` + Claude `inferred`/`high`; auto-resolve <2s; jump-focus; palette; mute/solo; restart-persistence; CLI+GUI+Hub same protocol); Windows best-effort smoke | acceptance |

**Coverage enforcement.** The numeric floors above are enforced in **CI** (`cargo llvm-cov`; rustup
runners install `llvm-tools-preview`) — see `.github/workflows/ci.yml` (workspace ≥80%, the
G0-frozen protocol+hub ≥85%). The **local** build-gate runs on a Nix toolchain without llvm-tools,
so it enforces the stronger *qualitative* bar instead: the required tests must **exist**, be
meaningful, and be green. A green local gate is **not** proof the numeric floor is met — that check
lives in CI.

**Cross-cutting per-node test duties** (every build node, not just gates): unit tests land **with**
the code (red→green in the same node), public APIs get doc-tests, and the node returns *green or it
returns failed* — a node never reports done with a red suite.

---

## 4. Parallelism ramp (how width increases)

| Wave | Unlocked by | Concurrent nodes | Why this width |
|---|---|---|---|
| W1 | start | `PROTO` → `HUB` → {`CLI`,`FAKE`} (1→2) | Everything depends on the protocol; near-serial by necessity |
| W2 | `◆G0` | {`REPCORE`,`PERSIST`,`UISHELL`} then `EXTSKEL`,`IDENTITY`,UI-slices (3→5) | A stable protocol unblocks reporter, persistence, and the UI shell in parallel |
| W3 | `◆G1` | detection band (`SHIM→CODEX`, `INIT→CLHOOK→{CLINFER,CLUSETERM}`, `READSTREAM`) **∥** faces band (`UISORT`,`UINOTIFY`,`UICONF`,`FOCUS`,`PALETTE`,`MUTE`) → **~10** | Reporter API frozen → adapters fan out; protocol frozen → faces fan out; the two bands are independent |
| W4 | `◆G2`,`◆G3` | `EDITORS` → `E2E` (2→1) | Integration must see the whole; converges to one acceptance gate |

Actual concurrency is capped at `min(16, cores−2)` by the runtime; the DAG defines the *available*
parallelism, which is what widens. Under a token budget, the workflow scales fixer-fan-out and
optional extra property-test passes to the budget (see script).

---

## 5. Execution model — the dynamic workflow

The graph above is executed by a **single DAG-runner workflow**: gates are modeled as nodes, so the
scheduler launches each node the instant its predecessors are green — parallelism *emerges* from the
edges and *widens* after each gate, with no hand-coded waves. Code-mutating nodes run in **worktree
isolation** (parallel writes don't collide); gate nodes run the suite and **loop-to-green or HALT**.

```js
export const meta = {
  name: 'build-fleet',
  description: 'Top-down, phase-gated build of Fleet v1; DAG scheduler, gates = heavy unit testing',
  phases: [
    { title: 'P0 Spine' }, { title: 'G0' }, { title: 'P1 Framework' }, { title: 'G1' },
    { title: 'P2 Detection' }, { title: 'P3 Faces' }, { title: 'G2' }, { title: 'G3' },
    { title: 'P4 Integrate' }, { title: 'G4 DoD' },
  ],
}

const NODE = {
  type: 'object', additionalProperties: false,
  required: ['node', 'files_touched', 'tests_added', 'suite_green', 'notes'],
  properties: {
    node: { type: 'string' }, files_touched: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'integer' }, suite_green: { type: 'boolean' },
    coverage_pct: { type: 'number' }, notes: { type: 'string' },
  },
}
const GATE = {
  type: 'object', additionalProperties: false,
  required: ['gate', 'pass', 'failures', 'coverage_pct', 'summary'],
  properties: {
    gate: { type: 'string' }, pass: { type: 'boolean' },
    failures: { type: 'array', items: { type: 'string' } },
    coverage_pct: { type: 'number' }, summary: { type: 'string' },
  },
}

// ---- the work graph (single source of truth; gates are nodes) ----
const G = [
  { id: 'REPO',  ph: 'P0 Spine', deps: [], brief: 'S0: Cargo+pnpm monorepo, CI matrix mac+linux' },
  { id: 'PROTO', ph: 'P0 Spine', deps: ['REPO'], brief: 'S1: fleet-protocol crate + JSON Schema + TS gen' },
  { id: 'HUB',   ph: 'P0 Spine', deps: ['PROTO'], brief: 'S2: Hub spine WS+unix(cfg) + lockfile + merge' },
  { id: 'CLI',   ph: 'P0 Spine', deps: ['HUB'], brief: 'S3: fleet ls' },
  { id: 'FAKE',  ph: 'P0 Spine', deps: ['HUB'], brief: 'S4: fake reporter / fixtures' },
  { id: 'G0', gate: true, ph: 'G0', deps: ['CLI', 'FAKE'], scope: 'protocol + hub + cli (+ FAKE two-face consistency)' },

  { id: 'REPCORE',  ph: 'P1 Framework', deps: ['G0'], brief: 'S5: reporter register/heartbeat/buffer/reconnect' },
  { id: 'IDENTITY', ph: 'P1 Framework', deps: ['REPCORE'], brief: 'S6: durable id — seq, ordered replay, atomic GC' },
  { id: 'PERSIST',  ph: 'P1 Framework', deps: ['G0'], brief: 'S7: SQLite event log + restart-restore + reap' },
  { id: 'EXTSKEL',  ph: 'P1 Framework', deps: ['REPCORE'], brief: 'S8: VS Code ext skeleton (Open-VSX ^1.93)' },
  { id: 'UISHELL',  ph: 'P1 Framework', deps: ['G0'], brief: 'S19: Tauri read-only inbox' },
  { id: 'G1', gate: true, ph: 'G1', deps: ['IDENTITY', 'PERSIST'], scope: 'reporter + identity + persistence' },

  { id: 'ENVINJ',     ph: 'P2 Detection', deps: ['EXTSKEL'], brief: 'S9: EnvironmentVariableCollection injection' },
  { id: 'SHIM',       ph: 'P2 Detection', deps: ['ENVINJ'], brief: 'S10: PATH-shim claude/codex (B′)' },
  { id: 'CODEX',      ph: 'P2 Detection', deps: ['SHIM', 'G1'], brief: 'S11-13: Codex hooks detect+approval+auto-resolve' },
  { id: 'INIT',       ph: 'P2 Detection', deps: ['G1'], brief: 'S14: fleet init/uninit config fallback' },
  { id: 'CLHOOK',     ph: 'P2 Detection', deps: ['INIT'], brief: 'S15: Claude working/idle/done via hooks' },
  { id: 'CLINFER',    ph: 'P2 Detection', deps: ['CLHOOK'], brief: 'S16: Claude inferred waiting + JSONL drift-guard' },
  { id: 'CLUSETERM',  ph: 'P2 Detection', deps: ['SHIM', 'CLHOOK'], brief: 'S17: Claude high-confidence in shim terminal' },
  { id: 'READSTREAM', ph: 'P2 Detection', deps: ['EXTSKEL'], brief: 'S18: read-stream OSC recovery' },

  { id: 'UISORT',   ph: 'P3 Faces', deps: ['UISHELL'], brief: 'S20: urgency + age + sort' },
  { id: 'UINOTIFY', ph: 'P3 Faces', deps: ['UISHELL'], brief: 'S21: OS notify + auto-resolve + sound tiers' },
  { id: 'UICONF',   ph: 'P3 Faces', deps: ['UISHELL'], brief: 'S22: confidence surfacing' },
  { id: 'FOCUS',    ph: 'P3 Faces', deps: ['UISHELL'], brief: 'S23: per-OS focus + confirmation telemetry' },
  { id: 'PALETTE',  ph: 'P3 Faces', deps: ['UISHELL'], brief: 'S24: fuzzy palette + cycle-unread' },
  { id: 'MUTE',     ph: 'P3 Faces', deps: ['UISHELL', 'HUB'], brief: 'S25: mute/solo Hub commands' },

  { id: 'G2', gate: true, ph: 'G2', deps: ['CODEX', 'CLINFER', 'CLUSETERM', 'READSTREAM'], scope: 'all detection adapters (fixtures + state-machine + drift fuzz)' },
  { id: 'G3', gate: true, ph: 'G3', deps: ['UISORT', 'UINOTIFY', 'UICONF', 'FOCUS', 'PALETTE', 'MUTE'], scope: 'all faces/UI reducers' },

  { id: 'EDITORS', ph: 'P4 Integrate', deps: ['FOCUS', 'G2'], brief: 'S26: multi-editor descriptor table + launch/focus' },
  { id: 'E2E',     ph: 'P4 Integrate', deps: ['G2', 'G3', 'EDITORS'], brief: '§21 DoD scenarios as integration tests' },
  { id: 'G4', gate: true, ph: 'G4 DoD', deps: ['E2E'], scope: 'v1 Definition of Done (§21 items 1-11, mac+linux)' },
]
const byId = Object.fromEntries(G.map(n => [n.id, n]))

async function buildNode(n) {
  return agent(
    `Implement node ${n.id} — ${n.brief} — per main/PLAN.md (+ docs/ENGINEERING_SPEC.md spec). ` +
    `HEAVY UNIT TESTING IS MANDATORY AND PART OF DONE: write the unit tests WITH the code, cover ` +
    `happy path + edge cases + failure modes, and DO NOT return until this unit's own crate/package ` +
    `tests are green (cargo test / pnpm test). Respect the locked decisions (D1–D18) and invariants ` +
    `(§3 of PLAN.md): observer-not-owner, licensing-clean, confidence-honesty, reversibility. ` +
    `Return the structured report; suite_green MUST reflect reality.`,
    { label: `build:${n.id}`, phase: n.ph, isolation: 'worktree', schema: NODE })
}

async function runGate(n) {
  const FIX_ROUNDS = budget.total ? Math.max(3, Math.floor(budget.remaining() / 200_000)) : 3
  for (let attempt = 1; attempt <= FIX_ROUNDS; attempt++) {
    const r = await agent(
      `PHASE GATE ${n.id}. Run the FULL unit-test suite for [${n.scope}] AND every prior phase ` +
      `(no regressions). Enforce the coverage floor and the property/fuzz tests named for this gate ` +
      `in main/WORK_GRAPH.md §3. Report pass=true ONLY if everything is green and the floor is met; ` +
      `otherwise list concrete failures (file::test — reason).`,
      { label: `gate:${n.id}:${attempt}`, phase: n.ph, schema: GATE })
    if (r.pass) { log(`◆ ${n.id} GREEN (cov ${r.coverage_pct}%)`); return r }
    log(`◆ ${n.id} RED attempt ${attempt}: ${r.failures.length} failures — dispatching fixers`)
    await parallel(r.failures.slice(0, 8).map(f => () =>
      agent(`Fix this gate failure without breaking other tests: ${f}`,
        { label: `fix:${n.id}`, phase: n.ph, isolation: 'worktree' })))
  }
  throw new Error(`Gate ${n.id} never reached green after ${FIX_ROUNDS} rounds — HALT`)
}

// ---- DAG scheduler: launch each node when its deps resolve; width emerges from edges ----
const memo = {}
function run(n) {
  if (memo[n.id]) return memo[n.id]
  memo[n.id] = (async () => {
    await Promise.all(n.deps.map(d => run(byId[d])))
    phase(n.ph)
    return n.gate ? runGate(n) : buildNode(n)
  })()
  return memo[n.id]
}

log('Building Fleet v1 as a phase-gated DAG; parallelism widens after each gate.')
const results = await Promise.all(G.map(run))
return { built: results.filter(Boolean).map(r => r.node || r.gate), final: 'G4' }
```

### Notes on the encoding
- **Gates-as-nodes** means the scheduler needs no hand-written waves: `◆G1` simply has many
  dependents, so the moment it's green ~10 nodes become runnable and the runtime fans them out.
- **`isolation: 'worktree'`** per build node so concurrent edits don't collide; a gate node reads
  the integrated tree (its scope) to run the suite. *(For real execution, the gate is also the
  natural merge point — see "open execution questions" below.)*
- **Loop-to-green-or-HALT** makes "heavy unit testing" a true gate, not a checkbox: the build
  cannot advance past red.
- **Budget-aware**: fixer rounds scale with `budget.remaining()`; pass a `+Nk` target to deepen.

---

## 6. Open execution questions (resolve before launching the build, not the design)
- **Merge strategy across worktrees at each gate** — node worktrees must be reconciled into the
  integrated tree the gate tests against. Options: gate agent merges its phase's worktrees; or build
  nodes commit to phase branches and the gate merges. *(Recommend: per-phase integration branch.)*
- **Human review cadence** — a fully-autonomous run to `◆G4` is large. Recommend **gate-by-gate
  review**: run to `◆G0`, you inspect, then release the next phase.
- **Re-verify-at-build-time** items from PLAN §6 (EnvironmentVariableCollection uninstall behavior,
  codex #25914) belong to their nodes' first task.

---

## 7. How to run
Launch the script in §5 via the Workflow tool. Recommended first cut: **stop after `◆G0`** (a pilot
that proves the spine + the gate machinery), review, then continue phase-by-phase. Full autonomous
run to `◆G4` is available but builds the entire system in one pass.

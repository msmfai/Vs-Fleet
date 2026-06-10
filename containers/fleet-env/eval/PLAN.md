# Fleet Behaviour-Test Harness — Implementation Plan

Goal: turn today's working foundation into a **broad, reproducible, headless
behaviour-test suite** for Fleet environments — many containers, many edge cases,
real VS Code actions driven and *verified to actually work*, with machine state
measured before/after. **This is manual-behaviour verification, not RL** (no
rewards/training) — we steal only the OSWorld "did the action produce the expected
effect" success-checker pattern and gym-style `reset/observe/act` ergonomics.

Built to be **implemented in parallel**: §3 freezes the contracts everyone codes
against; §5 is the dependency map; §6–§8 are the catalogs that split cleanly across
people/agents.

---

## 0. Current state (the foundation — DONE, on `build/fleet-v1`)

- `fleet-env:latest` image (`containers/fleet-env/Containerfile`): code-server
  (Open-VSX) + Claude Code + `fleet-bridge` + linux `fleet-reporter` + claude
  hook-wrapper. Built with **Docker + colima** (Apple `container build` is broken
  here; it *runs* images fine).
- Bridge is a two-way **observe/act** channel: `{type:"command", id, args, reqId}`
  → executeCommand (+ result); `{type:"query", reqId}` → state snapshot.
- `harness.mjs`: a bridge WS server + Playwright (opens each editor to bring its
  ext-host online) + `Env` (reset/observe/act/close) over N parallel containers.
- **Proven**: phone-home 3/3 parallel; `Terminal: New Terminal` → `terminalCount`
  0→1 (bash), procs 10→12, +58 MiB; screenshot evidence captured.
- Gotchas already solved (see §8): workspace-trust, `extensionKind: ["workspace"]`,
  host↔container networking, BuildKit context bug → Docker.

---

## 1. Goals & non-goals

**Goals**
- Dozens of environments in parallel, from known image states.
- A broad **behaviour catalog** (§6) — each drives a real action and asserts the
  effect via the bridge snapshot (not "command returned ok").
- A broad **edge-case catalog** (§7) — parameterized environments + failure modes.
- **State before/after** for every behaviour: VS Code state Δ + machine Δ (procs,
  mem, disk, fs changes, timing) + screenshot.
- One command runs the matrix (scenarios × behaviours), aggregates a report
  (human + JUnit + HTML-with-screenshots), cleans up guaranteed.

**Non-goals (for now)**
- No RL/reward/training loop. No agent *policy* — we drive scripted actions.
- Not the Fleet desktop app integration (tracked separately as stretch, §4 Track H).
- Not a public benchmark; this is our internal regression/behaviour suite.

---

## 2. Architecture

```
 run.mjs (orchestrator)
   ├─ BridgeHub (ws :51778)         ← every container's bridge dials in
   ├─ pool of Workers (bounded N)
   │    └─ Env  ── docker run fleet-env (scenario opts)
   │            ── Playwright page (brings ext-host online; screenshots)
   │            ── bridge conn (observe/act)
   │            ── machine probes (docker stats/diff/exec)
   ├─ Scenario registry  (§7)  ── how to reset() an env into a known state
   ├─ Behaviour registry (§6)  ── drive+assert one action
   └─ Reporter ── JSON + console + JUnit + HTML(+screenshots)
```

A **run** = for each (scenario, behaviour) pair in the selected matrix: reset an
env in that scenario, run the behaviour, record `{pass, detail, vscodeΔ, machineΔ,
screenshots, timings}`. Envs are reused across behaviours of the same scenario
where safe, or fresh per behaviour where isolation matters (behaviour declares).

---

## 3. FROZEN CONTRACTS — define + stub these FIRST (Track A, morning, blocks others)

Everything parallel codes against these. Land them as typed stubs before fan-out.

### 3.1 Behaviour
```ts
interface Behaviour {
  id: string;                       // "terminal.new"
  title: string;                    // "Terminal: New Terminal opens a terminal"
  tags: string[];                   // ["terminal","smoke"]
  isolation?: "fresh" | "shared";   // default "shared" (reuse env); "fresh" = own env
  scenarios?: string[];             // applicable scenario ids; default = all "base*"
  needs?: Capability[];             // bridge capabilities required (§3.3) — skip if absent
  run(env: Env): Promise<BehaviourResult>;
}
interface BehaviourResult {
  pass: boolean;
  detail: string;                   // human one-liner
  evidence?: Record<string, unknown>; // before/after snapshots, captured output…
  // machineΔ + screenshots are attached by the runner, not the behaviour
}
```

### 3.2 Env (the testable unit)
```ts
interface Env {
  id: string; name: string; port: number; scenario: Scenario;
  reset(): Promise<void>;                       // docker run (scenario opts) + open editor + wait bridge
  observe(tag?: string): Promise<Observation>;  // bridge snapshot + machine state (+ screenshot if tag)
  act(command: string, args?: unknown[]): Promise<unknown>; // executeCommand, throws on !ok
  request(msg: object): Promise<any>;           // raw bridge round-trip (for §3.3 actions/queries)
  exec(shCmd: string): string;                  // docker exec in the container
  screenshot(tag: string): Promise<string>;     // returns path
  close(): Promise<void>;                        // browser + docker rm (ALWAYS runs)
}
interface Observation { vscode: Snapshot; machine: MachineState; screenshot?: string; }
```

### 3.3 Bridge wire protocol (Track E owns impl; contract frozen here)
Server→bridge messages (all carry `reqId`); bridge→server replies `{type:"result",
reqId, ok, ...}`. **Capabilities** gate behaviours that need features not yet built.
```
ACTIONS                                   QUERIES
 command   {id, args}        (DONE)        query        {}                 → Snapshot (DONE)
 openFile  {path}                          fileContent  {path}            → {text}
 typeText  {text}                          terminalText {name?}           → {text}    (read buffer)
 termSend  {name?, text}                   diagnostics  {detailed:true}   → [{file,sev,msg}]
 writeFile {path, content}                 openEditors  {}                → [{path,active}]
 saveAll   {}                              setting      {key}             → {value}
 closeEditor {}                            extensions   {}                → [{id,active}]
```
`Snapshot` (extend over time): `{terminals[], terminalCount, activeEditor,
visibleEditors[], openTabs[], diagnostics:int}` (DONE) → add `editorText?,
selection?, statusBarItems?`.

### 3.4 Scenario (edge-case manifest)
```ts
interface Scenario {
  id: string;                       // "base", "large-repo", "mem-capped"
  title: string;
  image?: string;                   // default fleet-env:latest; §Track G variants
  docker?: { memory?: string; cpus?: string; env?: Record<string,string>; };
  setup?(env: Env): Promise<void>;  // git clone, write files, inject failure…
  expectBoot?: "ok" | "degraded" | "fail"; // for crash/edge scenarios
}
```

### 3.5 Result schema (Reporter consumes; freeze the JSON)
```jsonc
{ "run": {"startedAt","image","scenarios":N,"behaviours":M},
  "results": [ {"scenario","behaviour","pass","detail","evidence",
                "machineDelta":{"procs":"10→12","memMiB":58,"fsChanges":7},
                "timingsMs":{"act":120,"effect":900},"screenshots":["…/before.png"]} ],
  "summary": {"pass":N,"fail":M,"skipped":K,"durationMs":…} }
```

**Acceptance for Track A:** contracts above implemented as stubs; `run.mjs`
executes the existing 2 behaviours through the new registry/reporter producing the
JSON+console report; `node run.mjs --list` prints registered behaviours/scenarios.

---

## 4. Parallel workstreams (tracks)

Each track is independent once §3 lands. Format: scope · key files · DoD.

- **Track A — Framework** (FIRST, unblocks all). The §3 contracts; split `harness.mjs`
  into `lib/{bridgeHub,env,machine,report}.mjs`, `registry.mjs`, `run.mjs`; CLI
  flags (`--scenarios`, `--behaviours`, `--tags`, `--parallel`, `--keep`, `--list`,
  `--json out.json`); bounded worker pool; **free-port allocation** (no fixed
  8200+i); guaranteed cleanup (trap + finally). DoD: matrix runner + reporter green
  on existing behaviours.

- **Track B — Behaviour suite** (§6). Implement behaviours in `behaviours/*.mjs`,
  grouped by area; each self-contained, declares `needs`. Splittable across many
  agents (one area each). DoD: each area's behaviours pass on `base` scenario with
  evidence; flaky ones quarantined with `tags:["flaky"]`.

- **Track C — Edge-case scenarios** (§7). `scenarios/*.mjs`: repos to clone,
  resource caps, failure injection, multi-root, no-network. DoD: each scenario
  boots to its `expectBoot`; a smoke behaviour runs (or fails as expected) under it.

- **Track D — Observation depth**. Expand `machine.mjs` (disk, net, `docker diff`
  fs-change count, per-action latency) and `Snapshot` (editorText, selection,
  status bar). Visual: optional screenshot diffing / crop. DoD: every result row
  carries machineΔ + timings; opt-in visual-diff util.

- **Track E — Bridge protocol** (§3.3). Implement the new actions/queries in
  `packages/fleet-bridge/src/extension.ts` (+ bump vsix + rebuild image). Coordinate
  with A on capability advertisement (`hello` reports supported caps). DoD: each new
  action/query has a round-trip test; behaviours needing them unblock.

- **Track F — Scale / orchestration / reporting / CI**. JUnit XML + HTML report with
  linked screenshots; concurrency tuning + colima resource guidance; a `make
  eval`/`npm test`; retry-on-flake (configurable); container/image GC; a `--soak`
  mode (M rounds). DoD: `make eval` runs the full matrix at N≥10, emits all reports,
  leaves zero orphan containers.

- **Track G — Image matrix**. `fleet-env` build-args/variants: `+python`, `+node`,
  `+rust` (toolchain + matching VS Code extension from Open-VSX) for language
  behaviours (diagnostics/format); a `minimal` variant for resource tests. DoD:
  variant images build via Docker; Track C/B can target them by `scenario.image`.

- **Track H — Desktop integration** (STRETCH, separable). Point Fleet desktop's
  `spawn.rs` at `docker run` instead of host processes; rail shows container envs;
  native-menu command forwarding → the container bridge (now that observe/act works
  headlessly, the desktop path is the same protocol). DoD: spawn from the `+`
  button launches a container that appears + is drivable. *Owned by 1 person; not on
  the critical path for the test suite.*

---

## 5. Sequencing & parallelization map

```
              ┌──────────────── Track A (contracts+runner) ───────────────┐  ← morning, 1 owner
              │  (freeze §3, stub registry/reporter/run.mjs)               │
              └───────────────────────────┬───────────────────────────────┘
        once A's stubs land, fan out (all independent):
   ┌────────────┬────────────┬────────────┬────────────┬────────────┬─────────────┐
   B behaviours C scenarios   D observe    E protocol   F orchestr.  G images   H desktop
   (per area)   (per case)    depth        (+rebuild)   reporting    matrix     (stretch)
        │            │            │            │            │            │
        └──── B-behaviours that `needs` E actions wait for E (write against the frozen
              §3.3 contract now; flip on when E ships; until then they SKIP cleanly).
   C scenarios that `image:+lang` wait for G (same pattern: written now, skip until image exists).
```

Rules so parallel work never collides:
- One file per behaviour / per scenario → no merge conflicts; the registry just
  globs `behaviours/*.mjs` + `scenarios/*.mjs`.
- Nobody edits `harness.mjs` after A splits it; everyone adds files.
- E (protocol) is the only shared mutable surface — single owner, additive only.
- All new behaviours/scenarios land behind `tags`/`needs` so a partial suite is
  always green (skips, not failures).

---

## 6. Behaviour catalog (the parallelizable units — pick areas)

Each: **action → assertion (via snapshot/query)**. `*` = needs a Track-E capability.

**Terminal**
- `terminal.new` (DONE) — new terminal → terminalCount +1.
- `terminal.split` — split → +1, same group.
- `terminal.runEcho`* — send `echo FLEET_OK` → `terminalText` contains it.
- `terminal.kill` — open then kill → terminalCount back to 0.
- `terminal.cwd`* — run `pwd` → equals `/home/coder/project`.

**Files / editor**
- `file.create`* — `writeFile` + `openFile` → `activeEditor` is it; `editorText` matches.
- `file.openWelcomeClose` — close Welcome tab → `openTabs` shrinks.
- `editor.splitRight` — `workbench.action.splitEditor` → 2 visible editors.
- `editor.saveDirty`* — type text, saveAll → file on disk (`exec cat`) matches.
- `file.rename`* — rename via fs + reload → tab label updates.
- `quickOpen.byName`* — quick-open a known file → it becomes active.

**Search / replace**
- `search.findInFiles` — open search view → view visible.
- `search.replaceAll`* — seed a file, replace → `fileContent` reflects it.

**SCM / git**
- `git.initStageCommit`* — `git init` (setup) → stage+commit via commands → `exec
  git log` shows 1 commit.
- `git.diffDecorations` — modify tracked file → SCM shows 1 change.

**Views / panels / palette**
- `palette.open` (DONE) — showCommands ok.
- `view.toggleSidebar` / `view.togglePanel` — toggles reflected (status/visibility).
- `problems.open` — open Problems view.

**Diagnostics** (needs Track G `+lang` image for a language server)
- `diag.jsonSyntax`* — write invalid JSON → `diagnostics` count ≥ 1.
- `diag.pyflakes`* (+python) — write `import os\nx=` → diagnostics appear.

**Settings**
- `settings.toggleWordWrap` — toggle setting → `setting` query reflects it.

**Agent (ties to the Hub, the original point)**
- `agent.claudeRuns`* — `termSend` `claude -p "say hi"` → the env's **Hub session**
  shows a `working`→`idle` run (assert via `fleet ls`/Hub query within the harness).
- `agent.waitingState`* — a prompt that triggers an approval → Hub shows `waiting`.

**Input**
- `input.typeIntoEditor`* — `typeText` → `editorText` contains it.

(~25 behaviours; ≈5 areas × ~5 = clean split for ~5 parallel workers.)

---

## 7. Edge-case scenario catalog

- `base` (DONE) — empty `/home/coder/project`.
- `no-folder` — open with no `?folder` → behaviours that need a workspace SKIP.
- `small-repo` — clone a tiny repo in `setup`.
- `large-repo` — clone a big repo → measure boot time + mem (Track D timings).
- `many-files` — generate 5k files → file-watcher/search behaviour under load.
- `mem-capped` — `docker.memory: "512m"` → does it boot/degrade? `expectBoot:"degraded"`.
- `cpu-capped` — `docker.cpus:"0.5"`.
- `no-network` — `--network none` → reporter can't reach Hub (retries), editor still
  drivable; assert phone-home FAILS but commands WORK.
- `crash-boot` — bad `FLEET_WORKSPACE` / corrupt config → `expectBoot:"fail"`; harness
  reports cleanly (no hang).
- `multi-root`* — two folders → multi-root workspace behaviours.
- `+python` / `+node` / `+rust` (needs Track G images) — language diagnostics/format.
- `preexisting-agent` — `setup` starts a long `claude` run before behaviours → state
  is non-empty.
- `slow-fs`* — overlay/throttle (stretch).

---

## 8. Known gotchas (carry forward — don't re-debug these)

- **Build with Docker + colima**, never Apple `container build` (context-snapshot
  truncation, persists a corrupt ref across builder deletes). `colima start --cpu 4
  --memory 8` first. Apple `container` *runs* fine but we standardized on Docker.
- **Bridge needs `extensionKind:["workspace"]` + `capabilities.untrustedWorkspaces`
  AND the image disables `security.workspace.trust`** — otherwise the extension
  installs but silently never activates (no log).
- **The bridge only activates once a workbench client connects** → Playwright must
  open each editor; that's also the screenshot channel. Pure HTTP won't start the
  ext-host.
- **Networking (colima):** host→container = published `-p` ports; container→host =
  `host.docker.internal`; Hub/bridge bind `0.0.0.0` (`FLEET_WS_ADDR`,
  `FLEET_BRIDGE_ADDR`). Entrypoint honors `FLEET_HOST_ADDR`.
- **Readiness:** wait for code-server `302/200` (not "any byte"); retry Playwright
  `goto` (published port flaps for ~1s).
- **Harness hang trap:** a bare `wait` also waits on backgrounded servers — wait on
  specific PIDs. Verify the Hub is listening before polling `fleet ls` (else it
  hangs). Always free `:51778` before a run (kill stale bridges).
- **`fleet ls` cli binary is named `fleet`.** Hub is **ephemeral by default**
  (`FLEET_PERSIST` to opt into durable).
- **macOS xattrs** (`com.apple.provenance`) break some context loaders — Docker
  handles them; if anything chokes, `cat`-recreate or `xattr -rc`.

---

## 9. Definition of done (the suite)

- `make eval` (or `node run.mjs --parallel 12`) runs the full scenario×behaviour
  matrix headlessly, emits console + `eval.json` + `eval.xml` (JUnit) + `eval.html`
  (with screenshots), exits non-zero on any unexpected failure, leaves **zero**
  orphan containers/images/browsers.
- ≥20 behaviours across ≥6 areas; ≥8 scenarios incl. ≥2 failure modes + ≥1 language.
- Every result row carries a state-Δ (vscode + machine) and at least one screenshot.
- Documented in `containers/fleet-env/eval/README.md` (how to run, add a behaviour,
  add a scenario) so it's self-serve.

---

## 10. Tomorrow's kickoff order

1. **Track A** lands the §3 contracts + `run.mjs` + registry + reporter (existing 2
   behaviours green through it). ~½ day, blocks the rest.
2. Fan out B/C/D/E/F/G in parallel against the frozen contracts. E + G ship the
   capabilities/images that the `needs`/`image`-gated behaviours/scenarios flip on.
3. H (desktop) proceeds independently whenever a hand is free.

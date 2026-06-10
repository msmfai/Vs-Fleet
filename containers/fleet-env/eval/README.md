# Fleet behaviour-test suite

Headless, parallel, reproducible **behaviour tests** for Fleet environments. Each
test boots a containerized `fleet-env` (code-server + Claude Code + the Fleet
bridge), drives a *real* VS Code action through the bridge, and **asserts the effect
actually happened** by reading back a state snapshot — plus measures machine state
(procs / mem / fs changes) before and after and captures screenshots. This is
manual-behaviour verification (OSWorld-style "did the action produce the expected
effect"), **not** RL — no rewards, no training. See [`PLAN.md`](./PLAN.md) for the
full design and the §3 frozen contracts.

A **run** is the matrix of `(scenario × behaviour)`: for every selected scenario we
boot one env (free-port allocated) and run each applicable behaviour against it,
recording `{pass, detail, evidence, machineΔ, timings, screenshots}`.

---

## Requirements

- **Docker + colima** (never Apple `container build` — see `PLAN.md §8`).
  `colima start --cpu 4 --memory 8` before a run.
- The `fleet-env:latest` image built (`containers/fleet-env/Containerfile`), plus any
  language-variant images for `+python` / `+node` / `+rust` scenarios (Track G).
- Node deps already vendored in `node_modules/` (`ws`, `playwright`). If missing:
  `npm install` (or `make install`).

---

## Run it

```bash
make eval                  # full matrix, all reports, GC after. exits non-zero on any unexpected fail
make eval PARALLEL=12      # tune concurrency (bounded worker pool over scenarios)
make list                  # print every registered scenario + behaviour, then exit
make soak ROUNDS=5         # run the matrix N times back-to-back (stability / leak hunt)
make gc                    # remove orphan fleet-eval-* containers + dangling images
make clean                 # gc + wipe the artifacts dir
make help                  # the cheat-sheet above
```

Selectors (all optional, combinable):

```bash
make eval SCENARIOS=base,small-repo
make eval BEHAVIOURS=terminal.new,file.create
make eval TAGS=smoke,terminal
make eval KEEP=1           # leave containers up for debugging (no GC of the run's envs)
```

Retry-on-flake — re-run only the rows that failed, up to N times; a row that passes
on any attempt is folded back in as a **flaky pass**:

```bash
make eval RETRIES=2
```

`npm run` shortcuts are also wired (`npm test` / `npm run eval` → `make eval`,
plus `list` / `soak` / `gc` / `clean`).

You can also drive the orchestrator directly (the Makefile is a thin wrapper):

```bash
node run.mjs --parallel 12 --json artifacts/eval.json
node run.mjs --list
node run.mjs --scenarios base --behaviours terminal.new --keep
```

### Tuning concurrency

`PARALLEL` is the number of envs booted at once. Each env = one container + one
headless Chromium + one bridge connection, so budget roughly **~1 GiB + ~1 vCPU per
env**. With `colima --cpu 4 --memory 8`, `PARALLEL=4`–`6` is comfortable; push to
`10`+ only with a bigger colima VM. If boots start timing out (code-server never
serves `302/200`), lower `PARALLEL`.

---

## Reports / artifacts

Run artifacts land in `$(OUT)` (default `./artifacts`, override with `OUT=` or
`FLEET_EVAL_OUT`). The screenshot review page is written to `./index.html` at the
eval root so the latest visual result is part of the normal repo working tree:

| File        | What |
|-------------|------|
| `artifacts/eval.json` | the §3.5 result schema (machine-readable; the source of truth) |
| `artifacts/eval.xml`  | **JUnit XML** — one `<testsuite>` per scenario, one `<testcase>` per row; CI (GitLab/GitHub/Jenkins) ingests this for pass/fail/skip + per-test timing |
| `artifacts/eval.html` | **HTML report** — links to the captured PNGs in `artifacts/`; failures auto-expanded |
| `index.html` | **screenshot review page** — screenshot-first, keyboard-scrollable gallery with the row detail, rationale, provenance, machineΔ, timings, and evidence beside each image |
| `artifacts/*.png`     | per-behaviour before/after screenshots used by both HTML pages |

The console stream shows live `PASS / FAIL / SKIP / ERROR` per cell with the
machineΔ and timings. **Exit code is non-zero on any unexpected failure or error**
(a clean SKIP — e.g. an unmet bridge capability or a missing language image — is
*not* a failure).

How the artifacts get emitted: `run.mjs --json <path>` writes the JSON; the Makefile
additionally exports `FLEET_EVAL_JUNIT` / `FLEET_EVAL_HTML` (and `FLEET_EVAL_JSON`),
which `Reporter.finish()` honours to emit the XML/HTML without any extra flags. To
emit them from a bare `node run.mjs` invocation, set those env vars yourself:

```bash
FLEET_EVAL_JUNIT=artifacts/eval.xml \
FLEET_EVAL_HTML=artifacts/eval.html \
FLEET_EVAL_REVIEW=index.html \
  node run.mjs --parallel 6 --json artifacts/eval.json
```

### CI

`make eval` is the single entrypoint: it produces all three reports, GCs orphan
containers, and exits non-zero on unexpected failures. Point your CI at `eval.xml`
for the test report and publish `eval.html` plus `index.html` as browsable
artifacts. Use
`RETRIES=1`–`2` in CI to absorb the occasional published-port flap (`PLAN.md §8`)
without going red on a genuinely-passing behaviour.

---

## Adding a behaviour

A behaviour drives **one action** and asserts its effect via a snapshot/query — code
**only** against the §3 contracts in `behaviours/_contract.mjs`. The registry
auto-discovers every `behaviours/*.mjs` (files starting with `_` are ignored), so
you never edit a central list — just drop a file (or add to an area file like
`terminal.mjs`, `files.mjs`, …).

A module exports `export const behaviours = [ … ]`. Shape:

```js
// behaviours/myarea.mjs
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "myarea.thing",                 // unique, dotted
    title: "Does the thing and the effect shows up",
    tags: ["myarea", "smoke"],          // selectable via --tags / TAGS=
    // isolation: "fresh",              // optional: own env (default "shared" = reuse)
    // scenarios: ["base", "small-repo"], // optional; default = all "base*" scenarios
    // needs: ["writeFile", "fileContent"], // bridge caps required → SKIP if absent
    async run(env) {
      const before = await env.observe("myarea.thing.before"); // tag ⇒ screenshot
      await env.act("workbench.action.someCommand");           // throws on !ok
      await sleep(1500);                                        // let the effect land
      const after = await env.observe("myarea.thing.after");
      return {
        pass: /* assert the EFFECT, not "command returned ok" */ after.vscode.terminalCount > before.vscode.terminalCount,
        detail: `terminals ${before.vscode.terminalCount} → ${after.vscode.terminalCount}`,
        evidence: { before: before.vscode, after: after.vscode }, // shown in HTML/JSON
      };
    },
  },
];
```

Rules:

- **Assert the effect**, never "the command came back ok". Read it back via
  `env.observe()` (snapshot), `env.request({type:"fileContent", path})` (a §3.3
  query), or `env.exec("cat …")` (in-container shell).
- The runner attaches machineΔ + timings + a result screenshot — your `run()`
  returns only `{pass, detail, evidence?}`.
- If you need a bridge capability not yet shipped (anything beyond `command` /
  `query`), declare it in `needs:[…]`. The runner SKIPs cleanly when the env's bridge
  doesn't advertise it — **never hard-fail**. Same for behaviours that only make
  sense on certain scenarios (`scenarios:[…]`).
- Flaky? Tag it `["flaky"]` to quarantine, and rely on `RETRIES=`.
- Available `env` surface (§3.2): `reset / observe(tag?) / act(cmd,args?) /
  request(msg) / exec(shCmd) / screenshot(tag) / supports(cap) / close`.

Verify it registers without booting anything:

```bash
make list                 # your behaviour should appear
node --check behaviours/myarea.mjs
```

---

## Adding a scenario

A scenario is an edge-case manifest: how to `reset()` an env into a known state
(image, resource caps, env vars, setup steps, expected boot outcome). Code against
`scenarios/_contract.mjs`; the registry auto-discovers every `scenarios/*.mjs`.

```js
// scenarios/myscenario.mjs
/** @type {import("./_contract.mjs").Scenario[]} */
export const scenarios = [
  {
    id: "small-repo",
    title: "A tiny repo cloned into the workspace",
    image: "fleet-env:latest",          // default; use a +lang variant for diagnostics
    // docker: { memory: "512m", cpus: "0.5", env: { FOO: "bar" }, network: "none" },
    expectBoot: "ok",                   // "ok" | "degraded" | "fail"
    async setup(env) {
      // Runs AFTER the env is live (bridge connected). Use env.exec for shell.
      env.exec("git clone --depth 1 https://example/tiny /home/coder/project/tiny");
    },
  },
];
```

Notes:

- **By default a behaviour only runs on `base*` scenarios.** To run a behaviour
  under your scenario, list your id in that behaviour's `scenarios:[…]`.
- `expectBoot:"fail"` scenarios (corrupt config, etc.) are reported as an *expected*
  pass when boot fails — the harness never hangs on them.
- `docker.network:"none"` envs can't be reached over HTTP; such scenarios assert via
  `env.exec(...)` only (no Playwright page / screenshots).
- A scenario needing a not-yet-built variant image: write it now; it will SKIP/boot-
  fail cleanly until Track G ships the image. Keep the suite green via skips, not
  failures.

Confirm:

```bash
make list                                 # your scenario should appear
make eval SCENARIOS=small-repo TAGS=smoke # boot it + run a smoke behaviour
```

---

## Repo layout

```
eval/
  run.mjs            orchestrator + CLI (matrix, worker pool, free ports, cleanup trap)
  registry.mjs       auto-discovery: globs behaviours/*.mjs + scenarios/*.mjs
  Makefile           make eval / list / soak / gc / clean  (Track F)
  package.json       npm run shortcuts
  lib/
    bridgeHub.mjs    WS server the in-container bridges dial into (observe/act)
    env.mjs          the Env unit: docker run + Playwright page + bridge + probes
    machine.mjs      machine-state probes (procs/mem/fs/timing) for the before/after Δ
    report.mjs       Reporter: console + JSON + JUnit XML + linked screenshot HTML  (Track F)
  scripts/
    failed-ids.mjs   extract failed behaviour ids from a report (retry-on-flake)
    merge-report.mjs fold a retry report over the base (flaky → pass)
  behaviours/        one file per area; each exports `behaviours = [...]`  (_contract.mjs = the frozen shapes)
  scenarios/         one file per edge case; each exports `scenarios = [...]`
```

---

## Troubleshooting

- **Stale bridge port `:51778`** — a previous run left the hub bound. Free it before
  re-running. `make gc` clears orphan *containers*, not host sockets.
- **Boots time out / port flaps** — colima quirk (`PLAN.md §8`); lower `PARALLEL`, or
  add `RETRIES=`. The harness waits for a real `302/200`, then retries the Playwright
  nav a few times.
- **Everything SKIPs** — the bridge only advertises `command` / `query` until Track E
  ships more capabilities; behaviours with `needs:[…]` beyond those skip by design.
  Same for `+lang` scenarios before Track G images exist.
- **Orphan containers after a crash** — `make gc` (removes all `fleet-eval-*`).
  `make clean` also wipes `$(OUT)`.
- **The bridge never connects** — the ext-host only starts once a workbench client
  opens; Playwright must open the editor (it also is the screenshot channel). Confirm
  the image disables `security.workspace.trust` and the bridge has
  `extensionKind:["workspace"]` (`PLAN.md §8`).
```

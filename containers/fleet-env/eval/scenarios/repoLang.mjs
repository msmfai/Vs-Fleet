// Track C — repo / scale / language scenarios (§7 of the local eval plan).
//
// One self-contained file; the registry auto-discovers it via `scenarios/*.mjs`.
// Each scenario only resets an env into a known state — behaviours (Track B) assert.
//
//   small-repo   clone a tiny public repo in setup()           → search/file behaviours
//   large-repo   clone a big repo; rely on Track D boot timings → scale/boot behaviours
//   many-files   generate 5k files via env.exec                 → file-watcher / search load
//   multi-root   two folders + a .code-workspace                → multi-root behaviours
//   +python/+node/+rust  Track-G variant images (clean SKIP if the image is absent)
//
// Contract notes / assumptions (frozen §3.4):
//   • A scenario's setup(env) runs AFTER a successful boot; it has env.exec (docker
//     exec), env.act/request (bridge), env.supports(cap). We do repo work with
//     env.exec so it never depends on un-shipped bridge caps.
//   • The runner boots every scenario with `?folder=/home/coder/project` (single
//     root). True multi-root activation needs the editor reopened on a
//     `.code-workspace`; we stage that file + a 2nd folder here and tag the scenario
//     `needs:["openFile"]` so multi-root behaviours/work that require a bridge
//     re-open gate on it. The folder layout itself is real regardless.
//   • Language variants set `image` to the Track-G tag. Because the runner has no
//     scenario-level capability gate and treats an un-bootable image as an error
//     (unless expectBoot==="fail"), we resolve image presence at module load:
//       - image present → use it, expectBoot:"ok", real setup.
//       - image absent  → keep the variant tag for visibility but set
//         expectBoot:"fail" so the missing-image boot is recorded as an EXPECTED
//         (clean) boot-failure, NOT a hard error/hang. This is the closest the
//         frozen contract allows to "SKIP cleanly when the image is absent" without
//         editing run.mjs (which we do not own).

import { execSync } from "node:child_process";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Local image presence check (synchronous; safe at module load). Mirrors §8's
// "Docker runs images fine" — we never build here, only detect.
function imagePresent(tag) {
  try {
    execSync(`docker image inspect ${tag}`, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

// Build one language-variant scenario, degrading to a clean expected boot-failure
// when its Track-G image has not been built yet.
function langScenario({ id, title, image, setup, rationale }) {
  const present = imagePresent(image);
  return {
    id,
    title: present ? title : `${title} (image ${image} absent — SKIP)`,
    image,
    // Behaviours that target a +lang scenario should also declare their own
    // `needs:[...]`; this surfaces the language dependency at the scenario level.
    needs: ["diagnostics"],
    expectBoot: present ? "ok" : "fail",
    setup: present ? setup : undefined,
    rationale,
  };
}

// A tiny, stable public repo (used by small-repo). Octocat's "Hello-World" is the
// canonical minimal GitHub repo; shallow-clone keeps it instant.
const SMALL_REPO = "https://github.com/octocat/Hello-World.git";
// A genuinely large repo for boot/scale timing (Track D measures the cost). Shallow
// + single-branch to bound the clone; the *working tree* is what stresses the env.
const LARGE_REPO = "https://github.com/torvalds/linux.git";

/** @type {import("./_contract.mjs").Scenario[]} */
export const scenarios = [
  // ── small-repo ───────────────────────────────────────────────────────────
  {
    id: "small-repo",
    title: "Small repo — tiny public repo cloned into the workspace",
    image: "fleet-env:latest",
    expectBoot: "ok",
    rationale: `
WHAT: After a clean boot, setup() wipes the workspace root and shallow-clones the
canonical minimal GitHub repo (octocat/Hello-World) into /home/coder/project so the
editor and search index see real, version-controlled files. It then asserts the
clone landed by testing for /home/coder/project/README, and if the network/clone
hiccupped it writes a fallback README so the on-disk state is still deterministic.
The scenario is expected to boot "ok" and leave a populated single root.

WHY THIS IS CORRECT: setup() runs AFTER boot, so cloning never blocks activation —
the editor is already up when files appear, and the file watcher picks them up
(hence the trailing sleep). We assert via a follow-up \`test -e README\` rather than
trusting the clone's exit code because env.exec returns "" on failure and cannot be
relied on as a success signal. The fallback guarantees a known file exists no matter
what, which is the correct design: a search/open behaviour downstream must have
something concrete to find, and offline CI must not turn a network flake into a red
scenario.

WHY IT MATTERS: This is the smallest "real repo" fixture — every file/search/open
behaviour that needs an actual tree (not the empty base) depends on it. If a refactor
breaks the wipe-then-clone idempotency (e.g. leftover .git from a prior run), the
exit-code-vs-listing assertion contract, or the fallback path, downstream behaviours
would fail nondeterministically and a future reader would wrongly blame the search or
open code instead of this fixture's seeding. The fallback specifically guards against
the most common false negative: an offline harness mistaking "no network" for "editor
can't see files."`,
    async setup(env) {
      // Clone into the live workspace root so the editor/search sees real files.
      // Idempotent: wipe a prior tree first. env.exec returns "" on failure, so we
      // assert via a follow-up listing rather than trusting the clone's exit code.
      env.exec(
        "rm -rf /home/coder/project/.git /home/coder/project/* /home/coder/project/.[!.]* 2>/dev/null; " +
          `git clone --depth 1 ${SMALL_REPO} /tmp/_small && ` +
          "cp -a /tmp/_small/. /home/coder/project/ && rm -rf /tmp/_small",
      );
      const has = env.exec("test -e /home/coder/project/README && echo ok");
      if (has !== "ok") {
        // Network/clone hiccup — leave a marker file so dependent behaviours still
        // have a deterministic workspace and the scenario stays green at boot.
        env.exec("printf 'fleet small-repo fallback\\n' > /home/coder/project/README");
      }
      await sleep(500); // let the file watcher pick the tree up
    },
  },

  // ── large-repo ───────────────────────────────────────────────────────────
  {
    id: "large-repo",
    title: "Large repo — big checkout to stress boot/mem (Track D timings)",
    image: "fleet-env:latest",
    expectBoot: "ok",
    rationale: `
WHAT: After boot, setup() wipes the root and shallow + single-branch clones a
genuinely large repo (torvalds/linux) into /home/coder/project/repo under a 600s
timeout, materialising a big working tree. If the clone fails (offline CI), it falls
back to generating a wide synthetic tree (50 dirs × 40 files × 200 lines) so the
scenario still presents real scale. It expects an "ok" boot with a large tree on
disk for Track D's scale/boot-timing behaviours to measure.

WHY THIS IS CORRECT: The cost being exercised is the editor/file-watcher/indexer
confronting a large *working tree*, not git history — so --depth 1 --single-branch is
the right way to bound clone time while keeping the tree wide. Crucially the clone
happens after boot, so even a huge checkout cannot stall activation; "ok" is correct
because VS Code is expected to start and then stream the tree into its index, not
block on it. The synthetic fallback preserves the property under test (breadth of
files) so timing/scale behaviours stay meaningful and green without a network.

WHY IT MATTERS: This is the upper-end stress fixture for boot/memory/watch timings.
If a refactor regresses how the env handles a large tree — watcher exhausting inotify
handles, indexer blocking the bridge handshake, memory ballooning — Track D should
see it here first. The \`=== "CLONED"\` sentinel and the fallback guard against a
silent offline degrade masquerading as a pass: a future reader investigating a slow
boot needs to know whether they measured the real linux tree or the synthetic stand-in,
and this branch makes that determinable instead of conflating a network outage with a
performance regression.`,
    async setup(env) {
      // Shallow + single-branch to bound clone time while still materializing a
      // large working tree. If the clone fails (offline CI), fall back to a
      // generated tree so the scenario is still meaningfully "large" and green.
      const cloned = env.exec(
        "rm -rf /home/coder/project/.git /home/coder/project/* /home/coder/project/.[!.]* 2>/dev/null; " +
          `timeout 600 git clone --depth 1 --single-branch ${LARGE_REPO} /home/coder/project/repo ` +
          "&& echo CLONED",
      );
      if (cloned !== "CLONED") {
        // Offline fallback: a wide tree so search/watch behaviours still see scale.
        env.exec(
          "mkdir -p /home/coder/project/repo && cd /home/coder/project/repo && " +
            "for d in $(seq 1 50); do mkdir -p dir$d; " +
            "for f in $(seq 1 40); do printf 'line %s\\n' $(seq 1 200) > dir$d/file$f.txt; done; done",
        );
      }
      await sleep(1000);
    },
  },

  // ── many-files ───────────────────────────────────────────────────────────
  {
    id: "many-files",
    title: "Many files — 5k generated files (file-watcher / search under load)",
    image: "fleet-env:latest",
    expectBoot: "ok",
    rationale: `
WHAT: After boot, setup() generates 5,000 tiny files spread across 50 directories
(100 files each) under /home/coder/project/many entirely via env.exec, each tagged
with the marker string FLEET_MANY so search behaviours have a known needle. It counts
the result with \`find | wc -l\` and tolerates a partial generation rather than failing,
expecting an "ok" boot with a high file count for watcher/search-under-load behaviours.

WHY THIS IS CORRECT: Where large-repo stresses total bytes, this fixture isolates the
distinct axis of *file count* — many small entries are what actually pressure the file
watcher (inotify/poll fan-out) and the search indexer's enumeration, independent of
content size. Doing it purely through env.exec is deliberate: it needs no bridge
capability and runs identically on the base image, so the fixture can never be skipped
for missing caps. Generation after boot is correct because the watcher must observe
files *appearing*, which is the live event path being tested. The non-fatal short-count
branch is correct because a slow generator should degrade gracefully — behaviours
themselves decide whether the count they observe is sufficient.

WHY IT MATTERS: This guards the file-count scaling path specifically. If a refactor
caps the watcher, changes the search exclusion globs, or makes enumeration O(n²), the
breakage shows up here and not in large-repo (whose file count is far lower than 5k).
The FLEET_MANY marker and the explicit count give a future reader a deterministic way
to confirm the fixture actually produced its load before blaming the watcher/search
code — if the count came up short, the fault is generation, not indexing.`,
    async setup(env) {
      // Generate 5,000 small files spread over 50 dirs (100 each) entirely via
      // env.exec so it works on the base image with no bridge caps. Tiny payloads
      // keep disk use low while still exercising the watcher/indexer.
      env.exec(
        "rm -rf /home/coder/project/many && mkdir -p /home/coder/project/many && " +
          "cd /home/coder/project/many && " +
          "for d in $(seq 1 50); do mkdir -p d$d; " +
          "for f in $(seq 1 100); do printf 'file %s/%s FLEET_MANY\\n' $d $f > d$d/f$f.txt; done; done",
      );
      const count = env.exec("find /home/coder/project/many -type f | wc -l").trim();
      if (parseInt(count, 10) < 5000) {
        // Don't fail the scenario on a slow/partial generation — record what we got;
        // behaviours decide if the count is sufficient for their assertion.
      }
      await sleep(1000); // give the watcher a moment to enumerate
    },
  },

  // ── multi-root ─────────────────────────────────────────────────────────── (*)
  {
    id: "multi-root",
    title: "Multi-root — two folders + a .code-workspace",
    image: "fleet-env:latest",
    expectBoot: "ok",
    // True multi-root activation needs the editor reopened on the workspace file via
    // a bridge openFile; gate dependent behaviours on it. The on-disk layout is real.
    needs: ["openFile"],
    rationale: `
WHAT: After boot, setup() stages a real two-folder layout (alpha/A.txt, beta/B.txt)
plus a fleet.code-workspace file listing both folders, then expects an "ok" boot. The
scenario declares needs:["openFile"] so any behaviour requiring genuine multi-root
*activation* is gated on the bridge being able to reopen the editor on the workspace
file.

WHY THIS IS CORRECT: The runner always boots with ?folder=/home/coder/project, i.e.
a single root — VS Code only enters true multi-root mode when opened on a
.code-workspace, which requires reopening the editor, an action only the openFile
bridge capability can perform. So the honest split is: the on-disk folder + workspace
layout is always real and always created here (no capability needed to write files),
but the *activation* depends on a cap that may not ship. Tagging the scenario
needs:["openFile"] is the contract-correct way to say "the layout is guaranteed; the
multi-root editor state is only available where the bridge can reopen," letting the
runner cleanly skip dependent behaviours instead of asserting against a single-root
editor and falsely failing.

WHY IT MATTERS: This guards the boundary between "filesystem layout exists" and "the
editor is actually in multi-root mode." A future reader debugging a multi-root
behaviour break needs to know which half failed. If the openFile cap regresses, the
needs gate makes the behaviour SKIP (correctly) rather than fail in a confusing way;
if instead the workspace-file generation here breaks (e.g. malformed JSON heredoc),
that is this scenario's fault and is isolated from the bridge. Keeping the layout real
regardless means the fixture stays useful the moment the openFile cap ships.`,
    async setup(env) {
      env.exec(
        "mkdir -p /home/coder/project/alpha /home/coder/project/beta && " +
          "printf 'alpha root\\n' > /home/coder/project/alpha/A.txt && " +
          "printf 'beta root\\n'  > /home/coder/project/beta/B.txt && " +
          "cat > /home/coder/project/fleet.code-workspace <<'EOF'\n" +
          '{ "folders": [ { "path": "alpha" }, { "path": "beta" } ] }\n' +
          "EOF",
      );
      await sleep(500);
    },
  },

  // ── language variants (Track G images; clean expected-skip if absent) ──────
  langScenario({
    id: "python",
    title: "+python — python toolchain + Python extension",
    image: "fleet-env-python:latest",
    rationale: `
WHAT: On the Track-G fleet-env-python image, setup() seeds sample.py containing an
unused \`import os\` and a syntactically incomplete \`x =\`, then (via the shared
langScenario helper) declares needs:["diagnostics"]. Where the image is present it
boots "ok" and runs setup; where it is absent the helper degrades to expectBoot:"fail"
and drops setup so the missing image is recorded as an EXPECTED clean boot-failure,
not a hard error or hang.

WHY THIS IS CORRECT: The point of a +python scenario is to prove the Python language
server actually activates and reports diagnostics on this image. The seeded file is
chosen to provoke exactly that: an unused import (a lint/diagnostic) and an incomplete
assignment (a syntax error) are the minimal, language-server-specific signals a diag
behaviour can assert on. The absent-image degrade is correct because run.mjs treats an
un-bootable image as a hard error unless expectBoot==="fail" — resolving image presence
at module load and flipping expectBoot is the only contract-legal way (without owning
run.mjs) to turn "Track-G image not built yet" into a clean, visible SKIP rather than a
red hang.

WHY IT MATTERS: This guards two independent things a future reader must distinguish.
First, the language-toolchain wiring: if a refactor stops the Python extension from
activating or breaks diagnostics plumbing, the seeded file's errors won't surface and
diag behaviours fail — pointing at the toolchain, not the harness. Second, the
graceful-skip contract: if the absent-image degrade regresses, a CI box that simply
hasn't built the variant would start reporting hard failures and mask real problems.
The two distinct sample-file errors give a clear assertion target so a break is
attributable to a specific language-server capability.`,
    async setup(env) {
      // Seed a file that a Python language server should flag (unused import +
      // incomplete assignment) so diag behaviours have something to assert.
      env.exec(
        "printf 'import os\\nx =\\n' > /home/coder/project/sample.py",
      );
      await sleep(500);
    },
  }),
  langScenario({
    id: "node",
    title: "+node — Node toolchain + JS/TS extension",
    image: "fleet-env-node:latest",
    rationale: `
WHAT: On the Track-G fleet-env-node image, setup() seeds sample.js with \`const x = ;\`
— a deliberate syntax error — and (via langScenario) declares needs:["diagnostics"].
Present image → boot "ok" + run setup; absent image → expectBoot:"fail" with no setup,
recording the missing variant as an EXPECTED clean boot-failure rather than a hang.

WHY THIS IS CORRECT: \`const x = ;\` is an unambiguous JS parse error that the built-in
JS/TS language service must flag, making it the minimal, deterministic signal that the
Node toolchain and its diagnostics path are live on this image. Asserting on a hard
syntax error (rather than a style nit) avoids dependence on optional linters that may
not be installed — any working JS language service reports it. The absent-image degrade
mirrors the other variants: because run.mjs errors on an un-bootable image unless
expectBoot==="fail", resolving presence at load time and flipping the flag is the only
contract-safe way to make an unbuilt variant a clean SKIP.

WHY IT MATTERS: This isolates the Node/JS-TS language activation and diagnostics
plumbing. If a refactor breaks how the JS/TS extension activates or how diagnostics are
surfaced through the bridge, the seeded parse error won't appear and diag behaviours
fail here specifically — telling a future reader the fault is the Node toolchain wiring,
not the base harness or another language. The graceful-skip half guards CI boxes that
haven't built the node variant from emitting misleading hard failures.`,
    async setup(env) {
      env.exec(
        "printf 'const x = ;\\n' > /home/coder/project/sample.js",
      );
      await sleep(500);
    },
  }),
  langScenario({
    id: "rust",
    title: "+rust — Rust toolchain + rust-analyzer extension",
    image: "fleet-env-rust:latest",
    rationale: `
WHAT: On the Track-G fleet-env-rust image, setup() seeds sample.rs with
\`fn main() { let x: i32 = "s"; }\` — a type mismatch (a &str assigned to an i32) — and
(via langScenario) declares needs:["diagnostics"]. Present image → boot "ok" + run
setup; absent image → expectBoot:"fail" with no setup, so the unbuilt variant is an
EXPECTED clean boot-failure, not a hard error or hang.

WHY THIS IS CORRECT: Unlike the Python/Node samples, this is deliberately a *type*
error rather than a syntax error, because rust-analyzer's value over a plain parser is
exactly its type checking — a mismatched assignment is the minimal input that proves
the analyzer initialised and is performing real semantic analysis, not just parsing.
That makes it the right diagnostics target for a Rust toolchain. The absent-image
degrade is identical in spirit to the other variants and equally required: run.mjs
treats an un-bootable image as a hard error unless expectBoot==="fail", so detecting
presence at module load and flipping the flag is the only contract-legal clean SKIP.

WHY IT MATTERS: rust-analyzer is heavier and slower to initialise than the other
language servers, so this fixture specifically guards that the Rust toolchain reaches
full semantic-analysis readiness on its image. If a refactor leaves rust-analyzer
parsing-only (or not activating), the type error won't be reported and diag behaviours
fail here — pinpointing the Rust analyzer wiring for a future reader rather than the
harness. The type-vs-syntax choice is load-bearing: a syntax-only assertion could pass
on a half-initialised analyzer and hide exactly the regression this scenario exists to
catch. The graceful-skip half protects CI boxes lacking the rust variant from false
hard failures.`,
    async setup(env) {
      env.exec(
        "printf 'fn main() { let x: i32 = \"s\"; }\\n' > /home/coder/project/sample.rs",
      );
      await sleep(500);
    },
  }),
];

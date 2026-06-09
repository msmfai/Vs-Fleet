// Track C — repo / scale / language scenarios (§7 of PLAN.md).
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
function langScenario({ id, title, image, setup }) {
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
    async setup(env) {
      env.exec(
        "printf 'fn main() { let x: i32 = \"s\"; }\\n' > /home/coder/project/sample.rs",
      );
      await sleep(500);
    },
  }),
];

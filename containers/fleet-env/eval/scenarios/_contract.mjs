// Fleet behaviour-suite — FROZEN CONTRACTS (§3.4 of PLAN.md).
//
// Shared contract reference for every `scenarios/*.mjs`. JSDoc-only; the registry
// ignores files starting with '_', so this is never auto-discovered as a scenario.
//
// A scenario module exports `export const scenarios = [ /** @type {Scenario} */ … ]`.

// ─── §3.4 Scenario (an edge-case environment manifest) ───────────────────────
/**
 * @typedef {Object} DockerOpts
 * @property {string=} memory    e.g. "512m"  → docker --memory
 * @property {string=} cpus      e.g. "0.5"   → docker --cpus
 * @property {string=} network   e.g. "none"  → docker --network
 * @property {Record<string,string>=} env     extra container env
 */
/**
 * @typedef {Object} Scenario
 * @property {string}  id                       "base", "large-repo", "mem-capped"
 * @property {string}  title
 * @property {string=} image                    default "fleet-env:latest"; Track-G variants
 * @property {DockerOpts=} docker
 * @property {(env: import("../behaviours/_contract.mjs").Env) => Promise<void>=} setup
 *           git clone / write files / inject failure, run after boot
 * @property {("ok"|"degraded"|"fail")=} expectBoot   default "ok"
 * @property {string[]=} needs                  bridge caps required by setup; SKIP scenario if absent
 * @property {string}  rationale                REQUIRED. Full written rationale (multi-line ok): WHAT
 *                                              edge/condition this scenario reproduces, WHY its expected
 *                                              boot/behaviour (ok/degraded/fail) is correct, and WHY it
 *                                              matters (the real-world failure mode it guards). The
 *                                              harness auto-stamps the last-changed git commit+date.
 */

export {};

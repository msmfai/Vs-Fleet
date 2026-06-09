// Fleet behaviour-suite — FROZEN CONTRACTS (§3 of PLAN.md).
//
// This file is the shared contract reference for every `behaviours/*.mjs`. It
// exports nothing executable — only JSDoc @typedefs so every track codes against
// the same shapes. The registry (`registry.mjs`) ignores files starting with '_',
// so this file is never auto-discovered as a behaviour module.
//
// A behaviour module exports `export const behaviours = [ /** @type {Behaviour} */ … ]`.

// ─── §3.3 Bridge capabilities ───────────────────────────────────────────────
// Names a behaviour may list in `needs:[...]`. If the env's bridge does not
// advertise a capability, the runner SKIPS the behaviour cleanly (never fails).
// `command` and `query` are always present (shipped). The rest gate Track-E work.
/**
 * @typedef {(
 *   "command" | "query" |
 *   "openFile" | "typeText" | "termSend" | "writeFile" | "saveAll" | "closeEditor" |
 *   "fileContent" | "terminalText" | "diagnostics" | "openEditors" | "setting" | "extensions"
 * )} Capability
 */

// ─── §3.3 Snapshot (the VS Code state observed via the bridge `query`) ───────
/**
 * @typedef {Object} Snapshot
 * @property {string[]} terminals          names of open terminals
 * @property {number}   terminalCount      number of open terminals
 * @property {string=}  activeEditor       path of the active editor
 * @property {string[]=} visibleEditors    paths of visible editors
 * @property {string[]=} openTabs          labels/paths of open tabs
 * @property {number=}  diagnostics        total diagnostics count
 * @property {string=}  editorText         (Track-D / Track-E) text of active editor
 * @property {object=}  selection          (Track-D / Track-E) active selection
 * @property {object[]=} statusBarItems    (Track-D / Track-E) status bar items
 */

// ─── §3.2 MachineState (the host/container side-effect signal) ───────────────
/**
 * @typedef {Object} MachineState
 * @property {number} procs   process count in the container
 * @property {string} mem     human "used / limit" memory string
 * @property {string} cpu     CPU percentage string
 * @property {number=} fsChanges  (Track-D) docker-diff fs change count
 */

// ─── §3.2 Observation ────────────────────────────────────────────────────────
/**
 * @typedef {Object} Observation
 * @property {Snapshot}     vscode
 * @property {MachineState} machine
 * @property {string=}      screenshot   path, present when observe(tag) was called
 */

// ─── §3.2 Env (the testable unit; impl in lib/env.mjs) ───────────────────────
/**
 * @typedef {Object} Env
 * @property {string}   id
 * @property {string}   name
 * @property {number}   port
 * @property {Scenario} scenario
 * @property {() => Promise<void>}                       reset
 * @property {(tag?: string) => Promise<Observation>}    observe
 * @property {(command: string, args?: unknown[]) => Promise<unknown>} act
 * @property {(msg: object) => Promise<any>}             request
 * @property {(shCmd: string) => string}                 exec
 * @property {(tag: string) => Promise<string>}          screenshot
 * @property {() => Promise<void>}                        close
 * @property {(cap: Capability) => boolean}              supports
 */

// ─── §3.1 Behaviour ──────────────────────────────────────────────────────────
/**
 * @typedef {Object} BehaviourResult
 * @property {boolean} pass
 * @property {string}  detail                      human one-liner
 * @property {Record<string, unknown>=} evidence   before/after snapshots, output…
 */
/**
 * @typedef {Object} Behaviour
 * @property {string}   id                        e.g. "terminal.new"
 * @property {string}   title
 * @property {string[]} tags
 * @property {("fresh"|"shared")=} isolation      default "shared"
 * @property {string[]=} scenarios                applicable scenario ids; default all "base*"
 * @property {Capability[]=} needs                bridge caps required; SKIP if absent
 * @property {string}   rationale                 REQUIRED. A full written rationale (multi-line ok):
 *                                                WHAT this verifies, WHY that outcome was the expected/
 *                                                correct one, and WHY it matters (what regression it
 *                                                guards — these container tests break most on refactors
 *                                                and are painful to interrogate, so the "why" must live
 *                                                here). The harness auto-stamps WHEN it last changed
 *                                                (git commit+date of this file) — don't hand-write that.
 * @property {(env: Env) => Promise<BehaviourResult>} run
 */

export {};

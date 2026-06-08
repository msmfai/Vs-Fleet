# Fleet — Implementation Plan (v1)

> Companion to `README.md` (the canonical spec). This document resolves every
> `[IMPL]`/`[OPEN]` decision needed to start, and decomposes the spec's coarse
> Phase 0–3 into a long chain of **thin vertical slices**, each ending in a
> demonstrable *"everything works"* checkpoint. The goal is to maximize the number
> of intermediary states in which the whole system runs end-to-end.
>
> **Status:** decisions locked + **validated** (2026-06-08) via an 8-cluster research +
> adversarial-verify workflow. 15/18 decisions KEEP, 3 REVISE (D7, D8, D10), 3 unknowns RESOLVED.
> See §8. Slices S0–S26 = v1 (spec Phases 0–1).

---

## 0. Decisions locked before work begins

| # | Decision | Choice | Spec ref |
|---|---|---|---|
| D1 | Hub language | **Rust** — single static binary, shares Rust seam with Tauri host | §6.1 |
| D2 | Hub idle-exit | **Never auto-exit**; user quits explicitly; lockfile single-instance | §6.2 |
| D3 | Persistence | **SQLite** append-only event log + current-state projection | §16 |
| D4 | Durable identity | **Custom minimal** — reclaim-by-durable-id + buffered delta replay (no external broker) | §7.5 |
| D5 | Reporter language | **Rust where possible, TypeScript where required** (the VS Code extension is necessarily TS) | §9 |
| D6 | Wire format | **JSON** — human-debuggable through the hardest detection work | §7 |
| D7 | Transport | **WebSocket everywhere** (universal, cross-OS) **+ Unix-socket fast path on `cfg(unix)` only** (macOS/Linux). Windows fast path is out of v1 scope — Rust/tokio have no portable Windows `AF_UNIX` (rust-lang/rust#56533 open); Windows uses WS loopback. *Revised after validation* | §14.1 |
| D8 | First real agent | **Codex** (kept first) — earliest end-to-end proof, but high-confidence for a **hand-launched** TUI comes via **hooks** (`PermissionRequest`, default-on), not passive app-server. *Revised* | §8.2 |
| D9 | `done` state | **Kept distinct** from `idle` | §7.3, §22 |
| D10 | Codex channels | **Hooks-first** (`PermissionRequest`, default-on) for hand-launched TUIs; **app-server demoted to experimental**, gated on upstream multi-client (codex #25914). Passive app-server observation of a hand-launched TUI is maintainer-confirmed infeasible. *Inverted after validation — see §2* | §8.2 |
| D11 | v1 OS | **macOS + Linux first-class together**; Windows documented best-effort | §21, §22 |
| D12 | Repo | **Cargo + pnpm monorepo** (one repo, shared protocol crate) | — |
| D13 | License | **Private for now**; keep `fleet-protocol` cleanly separable for later OSS | §22 |
| D14 | VS Code extension | **Core Phase-0 component** (not optional) — the editor-scoped injection point (see §2) | §11.2, §8.1 |
| D15 | Owned-PTY power mode | **Excluded from v1** | §8.4 |
| D16 | Notifications | **OS-native via Tauri**, urgency-tiered | §15.2, §15.5 |
| D17 | Reap grace | **1 hour** (configurable) before `dead` GC | §7.3, §16 |
| D18 | Deploy placeholder | **`provisioning` placeholder tab**, matched by deploy-time durable id (Phase 2, but state machine accounts for it now) | §6.4 |

---

## 1. The cmux insight (why the extension is core)

Research into **cmux** (`manaflow-ai/cmux`, vendored as the orphan `cmux` branch / worktree)
established the closest prior art's mechanics:

- cmux gets owner-grade signal because it **owns the shell** — it spawns every terminal and
  injects `CMUX_SURFACE_ID`/`CMUX_SOCKET_PATH` env vars + a PATH-shimmed `claude` wrapper.
- It does **not** attach to independently-launched agents, and it does **not** tap a hand-launched
  Codex TUI's `app-server` JSON-RPC frames (its app-server client drives only sessions cmux
  launches in-process).

> **Validated against the actual cmux `main` source (not summaries).** Two prior-art claims the
> first pass overstated, now corrected: **(a)** the Codex integration that ships on `main` is a
> **CLI-driven `~/.codex/hooks.json` install** (`PreToolUse`/`PermissionRequest` → Feed approval,
> 120 s timeout) — the `Resources/bin/codex` wrapper + `codex-cmux-notify.sh` turn-complete reader
> are on the **unmerged PR #1482**, not shipped, so don't cite them as proven. **(b)** cmux's
> running `claude` wrapper injects `--session-id`/`--settings` but does **not** pass
> `--allow-dangerously-skip-permissions` (a test enforces its absence). That flag is a real Claude
> CLI flag we *may* use (S17), but it is **not** cmux-proven — and we must never silently default
> users into `bypassPermissions` (§3 invariant 3).

**Fleet refuses to own the terminal (§4.1/§4.5).** The one licensing-clean way to inject
identity + a wrapper into shells Fleet doesn't spawn is a **VS Code extension using the stable
`EnvironmentVariableCollection` API** (publishable, no proposed API, engine `^1.93.0`). That is
the sanctioned analog of cmux's env+PATH injection, done from inside the editor.

The extension therefore has **three jobs**, not the spec's one:
1. **Env injection** — `FLEET_SESSION_ID` (per window) + reporter socket path → per-window
   run↔editor correlation (also fixes focus/jump mapping, §12.2).
2. **PATH shim (B′)** — transparent `claude`/`codex` wrappers so the user *still types
   `claude`/`codex`* but gets hooks pointed at the reporter (+ for Claude, an injected
   `--session-id`/`--settings`). Any reliability flags are opt-in and surfaced, never silent
   (§3 invariant 3). cmux's wrapper trick, scoped to the editor, reversibly.
3. **Read-stream** — recover dropped OSC 9/777 via the stable shell-integration read-stream
   (the spec's original justification, §8.1).

**Confidence boundary (validated — technically sound, with two corrections folded in):**
- Agent launched in the editor's **integrated terminal** (shim applies) → **`confidence: high`**
  waiting detection. In plain-CLI mode Claude's `PermissionRequest` *does* fire (it only breaks in
  the native extension UI); this *is* the spec's "Use-Terminal mode", §8.1.
- Agent launched in a **native extension UI panel** (no shell, no shim) or **outside the editor**
  → only `Stop`/`UserPromptSubmit`/`PreToolUse` fire; **`Notification`, `PermissionRequest`, *and*
  `PostToolUse` do not** (reproduced through Claude ext v2.1.143, May 2026). So `working`/`idle`/
  `done` is reliable from `Stop`+`PreToolUse`, but waiting is **inferred** → **`confidence:
  inferred`**. *Correction: do not use `PostToolUse` for native-UI completion — it doesn't fire
  there either (#31285).*

Config-only `fleet init` (writing `~/.claude/settings.json` etc.) is the **graceful-degradation
fallback** for agents not under the shim.

---

## 2. Codex detection — RESOLVED (was the only open detection unknown)

**Question:** can Fleet passively observe a *hand-launched* Codex TUI via a shared standalone
`app-server` (high-confidence `turn/*` + `requestApproval`), or must it fall back to hooks?

**Resolved: NO — passive app-server observation of a hand-launched TUI is infeasible on stock
Codex**, now *maintainer-confirmed*, not merely an open RFC:
- Each client starts its own `app-server` instance; instances don't talk; **one active client per
  thread** (OpenAI maintainer closing RFC #21551 as completed, 2026-05-07; #15320 confirms
  concurrent multi-client thread access "isn't currently supported").
- The *routing* half exists (the TUI accepts `--remote ws://|unix://` + `--remote-auth-token-env`),
  but the *observation* half does not — a third client cannot tap another client's thread without
  an unmerged fanout fork. cmux corroborates: it taps app-server only for sessions it drives
  in-process.
- The only ways to get app-server signal both fail our constraints: (a) make Fleet the app-server
  *client* driving the thread → violates observer-not-owner; (b) carry a fragile unmerged patch.

**Therefore D10 is INVERTED:**
- **Default (hand-launched TUIs): Codex hooks.** Hooks are now **default-on** (the spec's
  "experimental / opt-in / Windows-disabled" framing is stale; canonical key `[features] hooks`,
  `codex_hooks` is a deprecated alias, Windows via `commandWindows`), with `PermissionRequest`
  first-class. This is what ships in cmux on `main`. Approvals come from `PermissionRequest`
  (+ `[tui] notifications` OSC9 as corroboration). **Note:** external `notify` fires only on
  `agent-turn-complete` (#19921), so it is *not* an approval channel — the fallback is
  "hooks + TUI OSC9", **not** "hooks + notify".
- **Experimental (gated): app-server-via-shared-server.** Revisit only when codex **#25914**
  closes or a multi-subscriber PR lands (*not* #21551/#15320 — already closed). Do **not** spike
  app-server-first; it would burn the spike.

This unknown is now closed; everything upstream (S0–S10) was independent of it regardless.

---

## 3. Cross-cutting invariants (every slice ends green on all of these)

1. **CI green on macOS + Linux** — `cargo build/clippy/test` + `pnpm -r build/lint/test`.
2. **Protocol versioned** — `schema_version` on every object; faces tolerate unknown fields.
3. **Observer, not owner** — keystrokes are never intercepted; we only shim the *launch
   environment*. No agent is launched *through* Fleet (owned-PTY off, §21.10).
4. **Licensing-clean** — no MS VS Code Server build / Marketplace / proposed APIs; extension is
   Open-VSX-publishable, engine `^1.93.0`.
5. **Confidence honesty** — every `waiting` carries `high|inferred` truthfully (§15.3).
6. **Reversible** — everything `fleet init` or the extension writes is backed up and undoable
   (`fleet uninit`, extension uninstall) (§8.5).
7. **Independently demoable** — each slice runs end-to-end by itself. No slice leaves the tree red.

---

## 4. The slice chain (S0–S26)

Each slice states **Goal**, **Build**, and **Demo = the "everything works" acceptance**.

### Group A — Protocol spine (round-trips end-to-end with no real agent)

- **S0 · Repo & CI skeleton.**
  Build: Cargo workspace (`fleet-protocol`, `fleet-hub`, `fleet-reporter`, `fleet-cli`) + pnpm
  workspace (`extension/`); CI matrix mac+linux.
  Demo: clean checkout builds, lints, tests — CI green.

- **S1 · Protocol crate v1.**
  Build: Rust types for `Session`/`AgentRun`/state/urgency/events/commands; serde JSON
  round-trip tests; emit a JSON Schema artifact; generate TS types from it.
  Demo: round-trip suite green; schema checked in; TS types compile against the same schema.

- **S2 · Hub skeleton.**
  Build: starts, binds WS + Unix socket, single-instance lockfile, detaches, accepts
  `subscribe`, returns empty `fleet.snapshot`, structured logging.
  Demo: a WS client connects → empty snapshot; a second Hub launch refuses (lockfile).

- **S3 · CLI face `fleet ls`.**
  Build: connects (WS or unix), subscribes, renders snapshot + applies live deltas.
  Demo: `fleet ls` shows an empty list and stays live.

- **S4 · Fake reporter → first whole-system green.** ⭐
  Build: `fleet-reporter --fake` registers one hardcoded session+run and scripts transitions
  (`working→waiting→working→dead`).
  Demo: scripted lifecycle appears identically in **two** `fleet ls` faces — proves §4.3.

### Group B — Reporter spine + identity + persistence (still no real agent)

- **S5 · Reporter skeleton + outbound registration.**
  Build: real reporter opens outbound WS/unix, registers, heartbeats, assigns run durable ids,
  buffers on disconnect, reconnects (still driven by fake transitions).
  Demo: start/stop reporter → session appears/disappears cleanly; kill+restore Hub → reconciles.

- **S6 · Durable identity / reclaim.**
  Build: reclaim-by-durable-id; reconnect replays buffered deltas, no ghost; reconnect vs
  fresh-start distinction; session-expiry GC. **Three locked invariants** (MQTT5-derived): (1)
  monotonic per-run `seq` on every delta, applied idempotently by `(durable_id, seq)`; (2) ordered
  replay by `seq` (last-writer-by-seq), not by arrival; (3) expiry GC drops the state entry **and**
  its buffered-delta queue **atomically**. Anchors: Codex `thread.id` (durable) and Claude
  `session_id` (validated stable across `--continue`/`--resume` on the current CLI) — no broker,
  no derived id.
  Demo: bounce the reporter mid-lifecycle → entry reclaimed, not duplicated; inject a duplicate +
  an out-of-order delta → no ghost, no regression; expired entry GC'd together with its buffer.

- **S7 · SQLite persistence + restart restore.**
  Build: append-only events table + current-state projection; restart rebuild; `dead` reaping
  after 1 h grace (D17). The D17 reap-timer plumbing is **reused** for the S6 session-expiry GC
  (atomic entry+buffer drop).
  Demo (§21.8): restart Hub and face → state intact; kill agent → `dead` with reason, reaped.

### Group C — Editor-scoped injection + Codex (first real agent)

- **S8 · VS Code extension skeleton.**
  Build: Open-VSX-publishable extension, engine `^1.93.0`, no proposed APIs; loads in VS Code +
  Cursor + Windsurf; connects to the reporter socket; heartbeat/status.
  Demo: extension activates in all three editors, talks to the reporter, shows as the editor face.

- **S9 · Env injection (`EnvironmentVariableCollection`).**
  Build: inject `FLEET_SESSION_ID` (per window) + reporter socket path into integrated-terminal
  shells; reversible.
  Demo: open an editor terminal → env present; uninstall → env gone; run↔window correlation works.

- **S10 · PATH shim wrappers (B′).**
  Build: prepend a PATH dir with transparent `claude`/`codex` shims; outside the editor the
  shims are absent (pass-through).
  Demo: `which codex` in the editor terminal → the shim; typing `codex` launches real codex,
  UX unchanged.

- **S11 · Codex working/idle/done via hooks (default path).** ⭐
  Build: via the shim, install `~/.codex/hooks.json` (`[features] hooks`, default-on):
  `SessionStart`/`UserPromptSubmit`/`PreToolUse` → `working`, `Stop`/turn-complete → `idle`/`done`;
  stable `thread.id`-anchored durable id. App-server is **not** used here (see §2 — infeasible for
  a hand-launched TUI; experimental path gated on codex #25914).
  Demo: a real **hand-launched** `codex` in the editor terminal → state flips
  `working↔idle↔done` live in `fleet ls`. **First real-agent checkpoint.**

- **S12 · Codex approval.**
  Build: the `PermissionRequest` hook → `waiting`+`approval`; its response → `working`. (App-server
  equivalents, for the future experimental path, are the namespaced
  `item/commandExecution/requestApproval` + `item/fileChange/requestApproval` + `serverRequest/resolved`.)
  Demo (§21.2): trigger a real Codex approval → tab shows `waiting`+`approval`.

- **S13 · Codex auto-resolve.**
  Build: answering in the real terminal drives resolved → working, clears `unread`, no Fleet
  interaction.
  Demo (§21.4): approve in terminal → state clears within 2 s.

### Group D — Claude detection (high in-terminal, inferred in native UI)

- **S14 · `fleet init` / `fleet uninit` (config fallback path).**
  Build: idempotent, reversible writer of Claude hooks + Codex config with backups.
  Demo: init→uninit round-trip leaves configs byte-identical to backup.

- **S15 · Claude working/idle/done via hooks→socket.**
  Build: hooks → reporter socket → states. **Reliable in *all* surfaces incl. native UI:**
  `Stop`/`UserPromptSubmit`/`PreToolUse`/`SessionStart`/`SessionEnd`. **Do NOT depend on
  `PostToolUse`** for native-UI completion — it does not fire there (#31285); derive `done` from
  `Stop`.
  Demo: hand-launched Claude → `working/idle/done` live, high confidence for covered states.

- **S16 · Claude inferred waiting (native UI / no-shim path).**
  Build: `PreToolUse`-without-`Stop` debounce + transcript-JSONL corroboration (a `tool_use`
  without its matching `tool_result`) → `waiting`+`approval` `confidence: inferred`. The JSONL line
  schema is community-documented and version-sensitive → **parse best-effort behind a schema-drift
  guard** that degrades gracefully rather than mis-stating.
  Demo: Claude approval in the native extension UI → `waiting`+`approval`, `confidence: inferred`.

- **S17 · Claude high-confidence in integrated terminal (shim path).**
  Build: Claude launched via the integrated-terminal shim (= Use-Terminal mode) with injected
  `--session-id`/`--settings` + reliability flags → `PermissionRequest` fires → upgrade to
  `confidence: high`.
  Demo (§21.3): same approval, shimmed integrated terminal → `confidence: high`.

- **S18 · Read-stream OSC recovery.**
  Build: extension uses stable `onDidStartTerminalShellExecution` + `read()` to recover dropped
  OSC 9/777 (Claude `Stop` OSC + coarse activity), corroborating state.
  Demo: an OSC the integrated terminal drops at the renderer is recovered and reflected in state.

> **Phase 0 DoD (§20):** `fleet ls` shows live, correctly-stated Codex + Claude sessions/runs
> with correct confidence flags — high for shimmed integrated-terminal runs, inferred for
> native-UI/external runs — surviving Hub + reporter restarts.

### Group E — Tauri host app (Phase 1; each slice a green UI checkpoint)

- **S19 · Tauri shell — read-only inbox mirrors `fleet ls`.**
  Demo: GUI list matches CLI live (proves §4.3 across CLI + GUI + Hub).

- **S20 · Urgency + age + sort.**
  Demo: a waiting tab rises to top by (unread, urgency, age) with a ticking waiting-age timer.

- **S21 · OS-native notifications + auto-resolving ping.**
  Build: Tauri v2 notification plugin. Implement urgency tiers **app-side via distinct desktop
  `sound` names** (omit `sound` for the silent `idle-done` tier) — do *not* rely on the `silent`
  field (iOS-scoped) or the Importance/channel API (Android-only). This resolves the urgency→
  loudness `[IMPL]` *mechanism* (exact tier→sound mapping is still a product choice).
  Demo (§21.2, §21.4): urgency-tiered notification fires; badge+notification clear automatically
  on terminal answer.

- **S22 · Confidence surfacing.**
  Demo (§15.3): `inferred` vs `high` rendered distinctly (hollow vs solid badge).

- **S23 · Editor focus / jump-to-next-unread.**
  Build: macOS — **`NSRunningApplication.activate` is unreliable on Sonoma+** (returns false / only
  bounces the dock; `ActivateIgnoringOtherApps` deprecated); use AppleScript `activate` or
  `NSWorkspace.openApplication`, validated on current macOS, **with focus-confirmation telemetry so
  the UI never falsely claims success**. Linux X11 — `wmctrl`/`xdotool`/EWMH. Wayland — activation
  of an already-running foreign window **cannot be guaranteed**; spawn/raise the editor via its own
  CLI passing a valid `XDG_ACTIVATION_TOKEN` + notification fallback, and never promise auto-focus.
  (No clean per-window "focus-this-workspace" VS Code URI exists → OS activation is always required.)
  Demo (§21.5): jump focuses the correct window + clears unread on mac + Linux X11; Wayland shows
  the documented fallback.

- **S24 · Fuzzy palette + cycle-unread.**
  Demo (§21.6): Cmd/Ctrl-K over all sessions focuses the match; cycle-without-clearing keybind.

- **S25 · Mute / solo.**
  Demo (§21.7): mute silences a session (state still live); solo silences all others — consistent
  across CLI + GUI.

- **S26 · Multi-editor launch/focus descriptor table.**
  Build: vscode/cursor/windsurf rows; show only installed; launch + focus.
  Demo (§21.1): ≥3 agents (Claude + Codex) across ≥2 editor windows (incl. a Cursor/Windsurf if
  installed) all appear within 2 s, correctly labeled.

> **v1 DoD:** full §21 list (items 1–11), macOS + Linux first-class, Windows documented
> best-effort.

---

## 5. Out of v1 scope (noted so the design accounts for them)

- Phase 2 — environments & deploy: code-server/openvscode-server images, Docker + Hetzner,
  phone-home over editor-provided reachability (§13/§14.2), `provisioning` placeholder (D18),
  per-location secrets (§19.3), phone view.
- Phase 3 — orchestration & automation: task queueing, inter-session deps, git-worktree option,
  usage/quota meter, rules engine (§17).
- Owned-PTY power mode (D15).

## 6. Remaining open items (resolve at the relevant slice)

- **S11 — RESOLVED** (§2): Codex hooks-first; app-server experimental, gated on codex #25914.
  Re-validate when that issue closes or a multi-subscriber PR lands.
- **S9/S10** — empirically re-verify current `EnvironmentVariableCollection` uninstall/dispose
  behavior on a recent VS Code build (the "env not cleared on uninstall" worry rests on vscode
  #234384, **closed completed Dec 2024**); keep the relaunch-terminal affordance only if stale env
  actually persists. Note the collection is **workspace-scoped, not per-terminal** (vscode #138109,
  closed by-design) — identity injection is coarser than cmux's per-surface scheme.
- **S21** — urgency→loudness `[IMPL]`: mechanism resolved (distinct `sound` names); exact
  tier→sound mapping still a product choice.
- **§22** — Windows first-class vs best-effort (locked best-effort; reinforced — no portable
  Windows `AF_UNIX`, rough Windows Codex-hook execution, deniable `SetForegroundWindow`).
- **§22** — open-sourcing protocol/Hub + license (post-v1; keep `fleet-protocol` separable).
- **Continuous** — monitor the **VS Code Agents window** (v1.120/1.121, May 2026), **Cursor 3
  Agents Window** (GA Apr 2026), and **Claude Agent View** each release. Differentiation is intact
  but *thinner* than Appendix B implies; the "won't *yet* appear" wording on third-party CLI
  sessions is the erosion signal to watch.

## 7. Reference material in-repo

- **`cmux` branch / worktree** — `manaflow-ai/cmux` vendored (orphan history) as the closest
  prior art. Shipped on `main` and studied: `docs/feed.md`, `docs/agent-hooks.md`,
  `docs/cli-contract.md`, `Resources/bin/cmux-claude-wrapper`, and the CLI-driven
  `~/.codex/hooks.json` install. **Not** shipped (unmerged PR #1482): `Resources/bin/codex`,
  `codex-cmux-notify.sh` — do not cite as proven precedent.

---

## 8. Validation pass (8-cluster research + adversarial verify, 2026-06-08)

A multi-agent workflow investigated every decision against current docs/source (web + the local
cmux source), with a skeptic re-checking each finding for version drift. **The architecture and the
differentiation thesis hold — no finding kills the project.** 15/18 decisions KEEP; **D7, D8, D10
REVISE** (folded in above); all 3 unknowns RESOLVED (§2 for S11; competition intact-but-thinner;
the high/inferred confidence boundary sound, with the two corrections folded into §1/S15/S16).

**Citation-hygiene backlog (mostly `README.md`, the canonical spec).** Many spec citations are
stale/closed/misattributed; the *substance* survives but the references must be re-anchored before
they mislead at build time:
- `microsoft/vscode #28338` → actually **`anthropics/claude-code #28338`** (closed not-planned);
  real upstream trackers are vscode #294247 (OSC 99) + microsoft/terminal #7718 (OSC 777).
  Future-proof the S18 parser for **OSC 99**.
- `#121926` (reuse-window-affects-all-windows) was **fixed Aug 2022** — drop it; the real gap is
  the absence of a per-window focus URI (vscode-discussions #2907/#149067).
- `onDidWriteTerminalData` successor proposal is **#145234**, not #131165.
- vscode #138109 (terminal-scoping, closed by-design), #234384 (clear-on-dispose, **closed
  completed**), #188235 (zsh prepend, **fixed**) — stop citing as open defects; re-verify live.
- Codex: spec cites 0.125.0 but current is ~0.134–0.138; **hooks are default-on**, canonical key
  `[features] hooks`, approval methods namespaced (`item/commandExecution/requestApproval`).
- Claude broken-hook trackers: the live ones are **#29928** (idle_prompt) / **#31285**
  (PostToolUse); several others (#11156, #13203) are now closed-as-duplicate but the bug still
  reproduces (ext v2.1.143).

*I can run a separate pass to correct these in `README.md` on request.*

# Fleet — Engineering Specification

> **Codename:** "Fleet" (placeholder — rename freely).
> **Document status:** Canonical living spec, **v1.0**. Supersedes v0.1.
> **Audience:** An external development team building this with no further access to the author. Every product-level ambiguity is resolved here on purpose. Conventions: **[IMPL]** = implementer's choice with a stated recommendation; **[OPEN]** = deliberately unresolved, see §22; **[GAP]** = a confirmed platform limitation you must design around, detailed in Appendix A.
> **How to read this:** §1–§5 are the why and the rules. §6–§8 are load-bearing — the architecture, the protocol, and the detection layer (which is the project's single biggest risk). §9–§19 are the components. §20 is build order, §21 the definition of done. Appendix A is the list of platform gaps that will otherwise be rediscovered painfully in week three; treat it as part of the contract.

---

## Changelog
| Version | Change |
|---|---|
| 0.1 | First skeleton. v1-local scope frozen; remote as extension points. |
| 1.0 | Full spec. Topology corrected to **phone-home registration** (sessions register into a Hub the client owns; the client does not dial out). Unit of tracking fixed as **a VS Code-Server environment ("session")**. Detection layer rewritten around confirmed research: Claude `Notification` **and** `PermissionRequest` hooks are broken in the VS Code extension UI; OSC 9/777 are dropped by the integrated terminal but recoverable via the **stable Shell Integration read-stream**; Codex **app-server JSON-RPC** is the authoritative Codex channel. Deployed environments standardized on **code-server/openvscode-server** (Microsoft VS Code Server licensing forbids the intended use). Host framework fixed as **Tauri**. Transport fixed as **WebSocket**. Durable identity fixed as **MQTT-style persistent sessions**. Wayland foreign-window focus marked impossible-to-guarantee. Competitive risk (VS Code **Agents window**) recorded. |

---

## 1. Summary
Fleet is a local-first **observability and orchestration layer for terminal-based AI coding agents** (primarily Claude Code and OpenAI Codex CLI; any CLI agent in principle) running across many VS Code-family editor environments and across many locations (local machine, Docker containers, remote/cloud hosts such as Hetzner). Each environment runs a VS Code Server and **phones home** to register with, and stream its state to, a local **Hub** that the user's client owns. The client aggregates all registrants into one sidebar: an inbox showing which sessions are waiting for the human, with notifications that **auto-resolve** when the user answers the agent in its real terminal. The client can also **deploy** new environments; once deployed they phone in like any other registrant. Fleet observes and switches; it does **not** host, replace, or interpose on the agents. Mental model: "Signal/WhatsApp for your agents."

## 2. Goals
1. **Aggregate** every agent session across every editor environment into one sidebar, regardless of location (local/container/remote).
2. **Notify** when any session needs attention, differentiated by urgency (approval > question > idle/done).
3. **Auto-resolve** a notification when the user answers that session in its own terminal — no separate acknowledgement.
4. **Switch** fast: jump-to-next-unread, fuzzy session palette, cycle gestures.
5. **Preserve the existing workflow completely.** The user keeps launching agents as they do today; Fleet adds a layer and removes nothing.
6. **Be OS-agnostic** for the host application (macOS, Linux, Windows).
7. **Deploy** environments (container/cloud) from the client; deployed environments phone in identically to hand-started ones.
8. **Be licensing-clean** end to end (no reliance on Microsoft's proprietary server build, Marketplace, or remote extensions in shipped/deployed components).

## 3. Non-goals (firm)
- **Not multiplayer.** Single-user. No shared sessions, no watching a teammate's fleet.
- **Not a VS Code fork.** Fleet drives stock, user-installed editors and open-source servers from the outside; it never ships a patched editor.
- **Not a runtime / not an agent.** Fleet observes and (later) launches the real agent CLIs; it never reimplements an agent.
- **Not a cloud service.** Local is always the source of truth. Remote/phone faces are additional renderers of local state, never a relocation of authority.
- **Not autonomy.** Auto-answering prompts is permitted only under explicit, per-session, *visible* rules (§17). Silent autonomy is a defect.
- **Not raw-TUI screen-scraping.** Detection reads deliberate signal channels (agent hooks, agent-emitted OSC notifications, agent control protocols), never the rendered TUI text, as a primary technique.

## 4. Design principles (override convenience)
- **4.1 Observer, not owner.** The terminal stays a real terminal; keystrokes go straight to the agent.
- **4.2 Local is the source of truth.** The Hub runs on the user's machine (or a small endpoint they control); all faces are clients of it.
- **4.3 The protocol is the product.** The central artifact is the session-state protocol (§7). The sidebar is renderer #1. Every other face (CLI, tray, phone) consumes the same protocol with no Hub changes.
- **4.4 Phone-home registration.** Environments register *into* the Hub; the client does not discover or dial out to environments. Deploy produces more registrants; it does not produce owned children with a special lifecycle.
- **4.5 The editor is the OS.** Fleet relies on VS Code-family remote/server features to make a container or remote host's filesystem behave as a local workspace. The reporter runs wherever the VS Code Server runs, so local/container/remote are one detection codepath. Location is metadata (a glyph + attach hint), not a separate transport Fleet implements.
- **4.6 Detection is source-side and out-of-band-first.** Do not depend on what a terminal *renders* or *forwards*. Detect from the agent's own signals (hooks, control protocol) delivered to the reporter directly. The in-editor terminal read-stream is a useful *additional* transport, not the foundation (§8).
- **4.7 Automation is opt-in and visible (§17).**
- **4.8 Licensing-clean by construction (§19).**

## 5. Domain model / glossary
- **Session** — one VS Code-Server environment Fleet tracks, identified by a stable `session_id`. Because the unit is a VS Code-Server environment, "an agent with no editor/server present" is simply not a Fleet session (this dissolves the old editor-gating question). A session contains one or more **agent runs**.
- **Agent run** — a single running agent process (`claude`/`codex`) inside a session, with its own native id (Claude `session_id`, Codex `threadId`) mapped to a Fleet run id.
- **Location** — where the session physically runs: `local | docker | remote`. Pure metadata + glyph + attach hint (§4.5).
- **Editor** — the VS Code-family client attached to the session (vscode | cursor | windsurf | none-yet), used for focus/jump.
- **State** — `working | waiting | idle | done | error | dead` (§7.2).
- **Urgency** — when `waiting`: `approval` (highest) | `question` | `idle-done` (lowest).
- **Reporter** — the per-environment process that detects agent state and phones home to the Hub (§9).
- **Hub** — the always-reachable endpoint that accepts registrations, holds canonical merged state, and serves faces (§10). Co-located with the client by default; relocatable per §10.2. (This is the artifact previously mislabeled "broker.")
- **Face / client / renderer** — any consumer of the Hub protocol (sidebar host app, CLI, tray, phone).

## 6. Architecture overview
```
   ENVIRONMENTS (each = a VS Code-Server "session", local/docker/remote)
   ┌───────────────────────────┐   ┌───────────────────────────┐
   │ Reporter (in-environment)  │   │ Reporter                  │
   │  - Claude hooks→local sock │   │  - Codex app-server (RPC) │
   │  - Codex app-server client │   │  - SI read-stream parse   │
   │  - shell-integration read  │   │  ...                      │
   │  - buffers + reconnects    │   │                           │
   └─────────────┬──────────────┘   └─────────────┬─────────────┘
   phone home (register + stream, outbound)        │
                 │  rides the editor's pseudo-local  │
                 │  connection where remote (§14)     │
                 ▼                                   ▼
            ┌──────────────────────────────────────────────┐
            │                     HUB                        │
            │  - accepts inbound registrations               │
            │  - canonical MERGED state (one truth)          │
            │  - append-only event log (persistence)         │
            │  - serves a subscribe/push protocol to faces   │
            │  - relays commands to reporters (focus, etc.)  │
            └───────────────▲───────────────┬────────────────┘
              subscribe      │               │ commands
                             │               ▼
            ┌────────────────┴───────────────────────────────┐
            │  FACES: sidebar host (Tauri) | CLI | tray | phone│
            └──────────────────────────────────────────────────┘
```
- **6.1 The Hub is mandatory and standalone**, a long-lived process separate from any editor or UI window (editor windows are mutually isolated — one extension host per window, no cross-window IPC [GAP A4] — so only an external Hub can hold a fleet-wide view). **[IMPL]** Implement the Hub in Rust or Go (single static cross-platform binary, good for a long-lived daemon).
- **6.2 Lifecycle.** First face or reporter to need it spawns the Hub if absent (lockfile/named-endpoint guards against duplicates); it detaches to outlive its spawner. Idle-exit policy **[IMPL default: never auto-exit; user quits explicitly]**.
- **6.3 Registration is inbound (§4.4).** Reporters open an outbound connection to the Hub and register; the Hub never initiates connections to environments.
- **6.4 Deploy → phone in.** When the client deploys an environment (§13), it **[IMPL]** immediately shows a placeholder tab in a `provisioning` state (so deploys are visible in flight); the tab transitions to live when the environment's reporter phones in and registers, matched by the durable id the client assigned at deploy time. *(Author's call; recorded as [IMPL] because the placeholder-vs-appear-on-registration question was never explicitly answered.)*

## 7. The protocol (the product — §4.3)
**7.1 The `Session` object.**
```json
{
  "session_id": "fleet-durable-uuid",
  "schema_version": 1,
  "title": "repo @ branch | user label",
  "location": { "kind": "local|docker|remote", "label": "...", "glyph": "laptop|docker|remote", "attach_hint": null },
  "editor":   { "kind": "vscode|cursor|windsurf|null", "focus_hint": "cli args / uri to focus this window" },
  "server":   { "kind": "code-server|openvscode-server|desktop-remote|local", "version": "..." },
  "runs": [ /* see 7.2 */ ],
  "rollup_state": "working|waiting|idle|done|error|dead",   // worst/most-urgent across runs
  "rollup_urgency": "approval|question|idle-done|null",
  "muted": false, "soloed": false, "unread": false,
  "tags": [], "policy": null,
  "updated_at": "ISO-8601"
}
```
**7.2 The `AgentRun` object** (a session has ≥1):
```json
{
  "run_id": "fleet-run-uuid",
  "agent_kind": "claude-code|codex|other",
  "native_id": "claude session_id | codex threadId",  // durable identity anchor (§7.5)
  "cwd": "/abs/path-at-location",
  "state": "working|waiting|idle|done|error|dead",
  "urgency": "approval|question|idle-done|null",
  "last_message": "short preview of agent output / its question",
  "waiting_since": "ISO-8601|null",
  "confidence": "high|inferred",   // 'inferred' when 'waiting' came from a heuristic, not an authoritative signal (§8)
  "diff_summary": null,            // v1.5+: {files_changed, insertions, deletions}
  "updated_at": "ISO-8601"
}
```
**7.3 State machine** (per run; session `rollup_*` = most-urgent across its runs):
```
 idle ──user submits prompt / tool starts──▶ working
   ▲                                            │ turn finishes (Stop / turn-complete)
   │ user answers in terminal (AUTO-RESOLVE)     ▼
 waiting ◀───agent asks approval/question──── working
   │   urgency = approval|question|idle-done
   ├── process exits / reporter gone ──▶ dead
   └── agent errors ──▶ error
```
- `waiting` is the only state that pings. Auto-resolve = the reporter observes the user's answer at the source and reports `working`; this clears `unread` and dismisses the notification with no Fleet interaction.
- `dead` runs/sessions remain (greyed, with reason) until dismissed; auto-reaped after a grace period and a session-expiry interval (§16) **[IMPL default: 1h]**.
**7.4 Events (Hub→face):** `fleet.snapshot` (full list on subscribe), `session.added|updated|removed`, `run.added|updated|removed`. Deltas preferred; full objects acceptable. **Commands (face→Hub):** `focus(session_id|run_id)`, `mute|unmute|solo(session_id)`, `dismiss(...)`, `set_tags(...)`, and (v1.5+) `deploy(spec)`, `launch_run(session_id, agent_spec)`. Commands the Hub can't satisfy itself are relayed to the relevant reporter/face.
**7.5 Durable identity & reconnection (MQTT-style — [GAP-free, but a known hard problem]).** Each run declares a fixed durable id anchored on its native agent id (Claude `session_id` / Codex `threadId`). The Hub keeps a **persistent session** keyed by that id across disconnects. On reconnect with the same id, the registrant **reclaims** the existing entry (no ghost duplicate); the Hub replays buffered deltas across the gap. Distinguish *reconnect* (reclaim, clean-start=false) from *fresh start* (new id, wipe). A **session-expiry interval** GCs entries whose registrant has been gone too long. **[IMPL]** Reuse MQTT persistent-session / "session present" semantics or an equivalent (NATS JetStream, Phoenix Channels resume). **[IMPL]** On reconnect, reclaim by durable id; *(author's call — the reconnect-identity question was never explicitly answered)*.
**7.6 Versioning.** `schema_version` on every object; faces tolerate unknown fields; breaking changes bump the version and the Hub advertises supported versions on connect.

## 8. Detection layer (THE core risk — build first, §20 Phase 0)
Confirmed findings (Appendix A) force a **hybrid, per-agent, multi-channel** design. The reporter (§9) runs co-located with the agent and assembles state from up to three channels. No channel alone is sufficient.

**8.1 Claude Code.**
- **Primary (authoritative-where-it-works): hooks → local socket.** Configure Claude Code hooks (HTTP or command type) to POST/write events to the reporter's local endpoint. **Reliable hooks to use: `Stop` (turn complete), `UserPromptSubmit` and `PreToolUse` (→ `working`, clears waiting), `PostToolUse`/`SessionStart`/`SessionEnd`.** Each event carries `session_id`, `cwd`, `tool_name`, `permission_mode`, `transcript_path`.
- **The waiting-for-approval problem [GAP A1]:** Claude's `Notification` (incl. `idle_prompt`/`permission_prompt`) **and** `PermissionRequest` hooks **do not fire in the VS Code extension native UI** (issues #11156, #16114, #28774, #29928, #31285, #59718 — all open/duplicate; a maintainer claim that PermissionRequest was fixed is refuted). The `idle_prompt` 60s idle was closed NOT_PLANNED even in the CLI (#8320). **Therefore the reporter must INFER "waiting for approval"** from the reliable signals: a `PreToolUse` with no matching `PostToolUse`/`Stop` within a debounce window → `waiting`+`approval` with `confidence:"inferred"`; corroborate via transcript-JSONL inspection (`~/.claude/projects/.../*.jsonl` tool_use without result). When the user runs Claude in the extension's **"Use Terminal" mode** (a plain CLI in the integrated terminal), `PermissionRequest` *does* fire — detect that mode and upgrade `confidence` to `high`.
- **Additional transport: shell-integration read stream (recovers dropped OSC).** VS Code's integrated terminal silently drops OSC 9/777 notification sequences at the renderer [GAP A2], but the **stable** Terminal Shell Integration API (`window.onDidStartTerminalShellExecution` + `TerminalShellExecution.read()`, stable since VS Code v1.93) lets a *publishable* extension read the raw output byte stream and recover those OSC sequences. This is the proven mechanism behind the `wenbopan.vscode-terminal-osc-notifier` extension (engine `^1.93.0`, no proposed API). Fleet's optional VS Code extension face (§11) uses this to recover Claude's `Stop`-emitted OSC and a coarse working/idle activity proxy. **Do not rely on `onDidWriteTerminalData`** — it is still a proposed API and cannot ship on the Marketplace [GAP A3].

**8.2 Codex.**
- **Primary (authoritative): the `codex app-server` JSON-RPC protocol.** The reporter runs/attaches to `codex app-server` (stdio, Unix socket, or experimental `--listen ws://127.0.0.1:PORT`) and consumes structured events: `turn/started`, `item/started|completed`, `turn/completed`, and the **server-initiated approval request** (`requestApproval`) + `serverRequest/resolved`. This yields precise `working` / `idle-done` / `waiting`+`approval` with `confidence:"high"`, independent of any terminal, with stable `threadId`/`turnId` and built-in resume. **This is Fleet's main Codex detector.**
- **Fallback (terminal-only users): `notify` + `[tui] notifications` + OSC.** Codex's external `notify` fires only on `agent-turn-complete` [GAP A2-codex]; for approval you need `[tui] notifications = ["agent-turn-complete","approval-requested","plan-mode-prompt"]` with `notification_method="osc9"`, `notification_condition="always"`, recovered via the §8.1 read-stream. Mark these `confidence:"inferred"` relative to the app-server.
- **Codex hooks** (`[features] hooks=true`: SessionStart/PreToolUse/PermissionRequest/PostToolUse/UserPromptSubmit/Stop; apply_patch + MCP coverage fixed via PRs #18391/#18385) are a secondary corroboration channel. Note: experimental, **disabled on Windows**, require trust.

**8.3 Channel-to-state reliability (design to this table).**
| State | Claude | Codex |
|---|---|---|
| turn complete / idle | `Stop` hook — **high** | app-server `turn/completed` — **high** |
| actively working | `PreToolUse`/`UserPromptSubmit` + stream activity — **good** | app-server `turn/started`/`item` — **high** |
| waiting for approval | **inferred** (PreToolUse-without-PostToolUse + transcript), or `high` in Use-Terminal mode | app-server `requestApproval` — **high** |
| session/run identity | hook `session_id` — **high** | app-server `threadId` — **high** |

**8.4 Owned-PTY power mode (opt-in only).** Fleet may offer a mode where it launches the agent through a PTY it owns (extension `createTerminal({pty})` or a node-pty child), giving 100% byte visibility incl. all OSC. **Off by default** — it violates §4.1/§4.5 (user must launch through Fleet) and adds TUI-forwarding burden. Gate behind a setting; never the default.

**8.5 Required environment config (reporter `fleet init` writes, idempotent, reversible):** Claude hooks in `~/.claude/settings.json`; Codex `[tui]`/`[features]` in `~/.codex/config.toml`; recommend enabling `terminal.integrated.shellIntegration.enabled` and a supported shell; recommend installing the OSC-notifier extension (or Fleet's own) for the read-stream face. Never silently overwrite user config; back up and support `fleet uninit`.

## 9. The reporter
One process per environment, co-located with the VS Code Server and the agents. Responsibilities: run the §8 detection adapters; assign/track run durable ids; maintain an **outbound** connection to the Hub (§14); **register** the session (§6.3) and stream deltas; **buffer and replay** across disconnects and **reconcile** on reconnect rather than reporting `dead` prematurely (§7.5); report `dead` only on confirmed process exit or reporter-gone timeout. The reporter is the unit that makes local/docker/remote identical (§4.5): it always runs "locally" relative to the agent, even when that locale is a container or a Hetzner box.

## 10. The Hub
**10.1** Accepts inbound registrations; holds the single canonical merged state (restoring "one truth" / §4.3 after the earlier per-environment-broker confusion was discarded); serves the subscribe/push protocol to all faces; relays commands; persists to the event log (§16). The Hub is dumb about detection — all fragile agent logic lives in reporters (§8) so it can change without touching the core.
**10.2 Location [IMPL, user pick 1c/2c → configurable, default local].** Default: the Hub runs on the user's primary machine alongside the client. Configurable to a small always-on endpoint the user controls (for when the primary machine sleeps/moves and remote environments still need a stable target). Not a third-party cloud.

## 11. Faces (renderers)
All consume the §7 protocol; none re-implement aggregation (the Hub already merged).
- **11.1 Sidebar host app (primary, v1).** Tauri (§18). The inbox UI (§15).
- **11.2 VS Code extension face (recovers in-terminal signals + reporter delivery).** A publishable extension that (a) injects per-window identity + spawns this window's reporter and installs the agent hooks/shim (delivery, **shipped v1**), and (b) uses the **stable** shell-integration read stream (§8.1) to recover dropped OSC and feed the reporter; also offers in-editor jump/focus. Engine `^1.93.0`, **no proposed APIs**.
- **11.2a In-editor inbox rail (Phase 1.5).** The §15 inbox rendered **inside** VS Code as an activity-bar view container (a `WebviewViewProvider`) — the Discord-style rail of sessions sitting to the left of the editor's own sidebar, with per-session unread/ping badges and click-to-switch. It is a *renderer* of the §7 protocol, identical in content to the §11.1 host window (same Hub, same view-model; no aggregation re-implemented). **Constraints (observer-not-owner):** it does **not** mirror or embed other editors' content — it shows status + switches focus to the real window. Because a VS Code extension only governs *its own* window, each window draws its own copy of the rail (all reading the same Hub), and "switch to session B" still resolves through the Hub + OS-level window activation (§12.2) — the same focus plumbing the host window uses. Per D14: stable APIs only (`WebviewViewProvider` is stable), Open-VSX-publishable.
- **11.3 CLI (v1.5).** `fleet ls`, `fleet go <id>`, `fleet mute <id>`, `fleet deploy <spec>`.
- **11.4 Tray/menubar mini-view (v1.5).** Unread count + quick jump for when the main window is closed.
- **11.5 Phone view (v2).** A protocol client over the Hub; local stays source of truth (§4.2).

## 12. Editor integration
**12.1 Descriptor table (data, not per-editor code).** Per editor: `kind`, `cli` (`code`/`cursor`/`windsurf`), `uri_scheme` (`vscode://`/`cursor://`/`windsurf://`), `open_flags` (`-r`/`--reuse-window`, `-n`/`--new-window`, `--folder-uri vscode-remote://...`), `detect`, focus strategy. Ship rows for all three; show only installed ones as launch targets.
**12.2 Focus [GAP A5].** There is no clean per-window "focus this exact workspace" URI (`--reuse-window` can affect all windows, #121926). Combine an editor CLI/URI open with OS-level window activation (§18.2).
**12.3 Cursor/Windsurf remote divergence [GAP A6].** "Editor is the OS" generalizes cleanly only to **Microsoft VS Code**. Since Microsoft restricted Remote-SSH/Dev-Containers to Microsoft editors (April 2025), Cursor/Windsurf rely on Open-VSX reimplementations (jeanp413 `open-remote-ssh`). Treat VS Code as the canonical remote host; support Cursor/Windsurf for **local** sessions fully and remote sessions best-effort, flagged. Their *native* agents (Composer/Cascade) are out of scope for detection (closed forks, not terminal CLIs) — Fleet still captures CLI agents running *inside* them.

## 13. Environments & deploy ("the editor is the OS")
**13.1 Server choice [licensing-critical, §19].** Deployed environments use **code-server or openvscode-server** with **Open-VSX**, NOT Microsoft's VS Code Server build (whose license forbids providing it as an integrated offering for others to use) [GAP A7]. The reporter + agent CLIs are baked into the image/instance.
**13.2 Locations.** `local` (v1, the degenerate case), `docker`, `remote` (v2). Tab glyphs for all three ship in v1 even though only `local` occurs, so v2 needs no UI change.
**13.3 Deploy flow (v2).** Docker: a `devcontainer.json`-based image (containers.dev) with server + agent + reporter pre-installed. Cloud (Hetzner): provision via Hetzner Cloud API / Terraform; cloud-init installs the image; the environment boots, the reporter phones in, the placeholder tab (§6.4) goes live. **Reuse existing patterns** (Coder Terraform templates incl. the Hetzner template, Daytona/DevPod Hetzner providers, Coder Codex module) rather than reinventing. Deploy is a client capability producing registrants (§4.4); it does not create a special owned lifecycle.

## 14. Transport
**14.1 Unifier: WebSocket.** One WebSocket/TLS protocol serves both (a) local faces subscribing to the delta stream and (b) inbound phone-home registrations from remote environments. Cross-OS (no Unix-socket vs named-pipe split). Optional **Unix-socket local fast path** only if profiling shows localhost WebSocket latency is a UX problem [IMPL]. Validated by cmux (Unix-socket scriptable API) and Codex app-server (JSON-RPC over socket/WS).
**14.2 Reaching the Hub from a remote/container environment [user pick 2a: ride the editor's pseudo-local connection].** The reporter's outbound connection rides the same transport that makes the workspace pseudo-local: the Hub's port is reachable inside the environment via the VS Code remote/SSH/tunnel port-forwarding the editor already establishes, so from the reporter's perspective the Hub is "localhost." **[GAP A8]** caveats: VS Code's auto port-forwarding is built for editor/browser use and Microsoft dev-tunnels are MS-operated with limits (≈10 tunnels/account, transfer caps) and are *not* a sanctioned third-party data plane. **[IMPL recommendation]** Treat "ride the editor's forwarding" as the default *path*, but implement it as a plain outbound WebSocket from the reporter to a Hub port that is reachable over whatever pseudo-local channel exists (forwarded port, SSH reverse tunnel `ssh -R`, or Tailscale if the user runs it) — i.e. depend on the *network reachability* the editor's connection provides, not on Microsoft's tunnel API specifically. Consistent with "session is the unit": if no VS Code-Server connection exists, there is no session to report (detached-watching is intentionally not a goal).
**14.3 Security.** TLS; token-authenticated registrations; loopback-only when purely local (§19).

## 15. Notifications & UX
**15.1 Inbox layout.** Vertical list of session tabs. Each: location glyph, agent-kind icon, title, state indicator, and when waiting: urgency color + **waiting-age timer** ("waiting 18m"). Waiting/unread sort to top by (unread, urgency, age). Age is load-bearing — the pain is the long-blocked session, not merely that something is blocked.
**15.2 Auto-resolving ping (§7.3).** Enter `waiting` → tab badges unread + notification fires (loudness per urgency). User answers in the real terminal → reporter reports `working` → badge + notification clear automatically. Manual `dismiss` exists only for `dead`/stale entries.
**15.3 Confidence surfacing.** A `waiting` with `confidence:"inferred"` (§8) is shown slightly differently (e.g. hollow vs solid badge) so the user knows it's a heuristic, not an authoritative approval signal — important given Claude's broken permission hooks.
**15.4 Mute / solo (per session).** Mute silences a session's pings (state still shown); solo mutes all but the soloed session(s). Hub commands (§7.4) so they're consistent across faces.
**15.5 Urgency tiers & focus modes.** `approval`=loud (sound + OS notification), `question`=OS notification, `idle-done`=silent badge [IMPL exact mapping]. Focus modes (v1.5): "approvals only", "DND except soloed", quiet hours.
**15.6 Switching ergonomics (first-class).** Jump-to-next-unread (one keybind, clears unread); cycle-unread-without-clearing (separate keybind); fuzzy session palette (Cmd/Ctrl-K over all sessions, all locations).

## 16. Persistence & lifecycle
Append-only local event log in the Hub (SQLite or append-only file; local only) backing: restart persistence (reopen the client → tabs + state restored from the still-running Hub or the log), cross-session search (v1.5+), and the transcript view (v-later). Reconnection reclaims by durable id (§7.5). `dead` reaping + session-expiry GC prevent ghosts.

## 17. Automation / rules engine (v-later — opt-in & visible, §4.7)
Staged, all off by default: (1) auto-answer pre-approved trivial prompts, per session; (2) glob-scoped policy (auto-answer *unless* the action touches configured path globs, e.g. never auto-approve under `prod/`); (3) escalation (page only after N retries). Policies are **per location** (a sandbox may be permissive; a remote prod box never is). Every automated action appears in the inbox as "Fleet answered this for you." Silent auto-action is a defect. This is the one area that touches the human-in-the-loop core value; keep it conservative, explicit, auditable.

## 18. Host app (Tauri)
**18.1 Framework: Tauri 2.x [resolved].** Small footprint (≈8–50 MB / 30–50 MB RAM vs Electron's larger), first-class tray, local IPC, Rust core that pairs with the Hub. The native window-activation work (§18.2) must be written natively regardless, so the Rust seam is not extra cost. Keep the host a thin protocol client.
**18.2 Cross-OS focus of external editor windows [GAP A5].**
- macOS: `NSRunningApplication.activate()` / AppleScript / URI — works.
- Windows: `SetForegroundWindow` gated by foreground-lock-timeout; reliable workaround is to synthesize an ALT keypress via `SendInput` immediately before, and/or `AttachThreadInput`/`AllowSetForegroundWindow`. Do not edit the global `ForegroundLockTimeout` registry value.
- Linux X11: `wmctrl`/`xdotool`/EWMH `_NET_ACTIVE_WINDOW` — works.
- **Linux Wayland: programmatic foreign-window activation is essentially impossible to guarantee** — apps can only *receive* focus via an activation token they were handed; you cannot raise an already-running external window. **Fallback:** spawn/raise the editor via its own CLI (passing `XDG_ACTIVATION_TOKEN` to the child), and rely on a tray/desktop notification rather than promising auto-focus. Document this limitation to users.

## 19. Security & licensing
- **19.1 No Microsoft proprietary components in shipped/deployed parts** [GAP A7]: do not bundle or connect Fleet to Microsoft's VS Code Server build, the MS Marketplace, or MS remote extensions. Use **code-server/openvscode-server + Open-VSX**. Driving the user's *own installed* desktop VS Code from the outside (launch/focus) is fine; shipping/operating Microsoft's server build is not.
- **19.2 Transport security:** TLS for any non-loopback connection; token-authenticated registrations; loopback-only binding when purely local.
- **19.3 Secrets per location (v2, design early):** an agent in a remote box needs tokens the local one shouldn't see; per-location credential scoping. Painful to retrofit — define the model before v2 deploy.
- **19.4 Tunnel limits:** if relying on VS Code/MS dev-tunnels for reachability, respect and document the account/tunnel/transfer limits; prefer user-owned reachability (forwarded port / SSH reverse tunnel / Tailscale).

## 20. Build order (phasing)
- **Phase 0 — Detection + Hub + protocol (DO FIRST; the core risk).** Hub (§10) + protocol with subscribe/push (§7) + reporters for Claude (hooks→socket + inference, §8.1) and Codex (app-server, §8.2) + `fleet init`. **Done when** `fleet ls` shows live, correctly-stated local sessions/runs updating in real time, with correct `confidence` flags.
- **Phase 1 — Sidebar host app (shippable v1, local-only).** Tauri host (§18), inbox with auto-resolving pings, urgency, age, confidence surfacing, mute/solo, jump-to-unread, fuzzy palette, multi-editor launch/focus (§12). Switching = window activation.
- **Phase 1.5 — Extra faces + QoL.** **In-editor inbox rail** — the Discord-style Fleet sidebar rendered *inside* VS Code as an activity-bar webview, left of the editor's own sidebar, with per-session pings and click-to-switch (§11.2a); read-stream recovery (§11.2), CLI, tray, tag grouping, focus modes, diff-summary, launch-run from sidebar.
- **Phase 2 — Environments & deploy.** code-server/openvscode-server images + reporter; Docker + Hetzner deploy (§13); phone-home over editor-provided reachability (§14.2); reporter resilience; per-location secrets; meaningful glyphs; phone view.
- **Phase 3 — Orchestration & automation.** Task queueing across sandboxes, optional inter-session dependencies, git-worktree option, usage/quota meter, rules engine (§17).
Each phase is independently useful. **Do not start Phase 1 until Phase 0 detection is reliable on current Claude Code and Codex versions** (re-verify the Appendix A issues at build time).

## 21. Definition of done — v1 (Phases 0–1)
1. Launch ≥3 agents (mix of Claude Code + Codex) across ≥2 editor windows (incl. a Cursor/Windsurf if installed); all appear in the sidebar within 2s, labeled with agent kind, title, cwd, and the `laptop` glyph.
2. Trigger a Codex approval prompt → its tab shows `approval` with `confidence:high` (via app-server), loud notification, rises to top with a running age.
3. Trigger a Claude approval in the extension UI → its tab shows `approval` with `confidence:inferred`; in Use-Terminal mode the same shows `confidence:high`.
4. Answer a waiting agent **in its own terminal** → badge + notification clear within 2s with no Fleet interaction.
5. Jump-to-next-unread focuses the right editor window (macOS/Windows/X11; Wayland shows the documented fallback) and clears its unread.
6. Fuzzy palette: type part of a title, Enter → that session's window focuses.
7. Mute silences a session (state still live); solo silences all others.
8. Close + reopen the Fleet window → session list restored (Hub kept running). Kill an agent → tab goes `dead` with reason, reaped after grace.
9. Hub + a CLI face + the sidebar all read the **same** protocol and reflect the same state live (proves §4.3).
10. The user's normal workflow is untouched throughout; no agent was launched through Fleet (owned-PTY mode off).
11. Host runs on macOS and Linux; Windows acceptable-with-documented-focus-caveats [OPEN §22].

## 22. Open questions (resolve before the relevant phase)
- **[OPEN]** Windows as first-class v1 vs documented best-effort (decides focus-strategy emphasis, §18.2).
- **[OPEN]** Whether `done` is a distinct state or collapses into `idle` (§7.3). *Lean: collapse.*
- **[OPEN]** Whether to consume Codex hooks in addition to app-server, or app-server only (redundancy vs simplicity).
- **[OPEN]** Whether to contribute fixes upstream (Codex `notify` for approvals; Claude permission hooks in the extension; a cmux/Agents-window equivalent) instead of carrying workarounds. Strategic, not blocking.
- **[OPEN]** Open-sourcing the protocol/Hub and license choice.
- **[OPEN]** Competitive response if VS Code's **Agents window** (§Appendix B) expands to subsume Fleet's niche — monitor each VS Code release.

## 23. Out of scope (restating)
Multiplayer; a VS Code fork; reimplementing any agent; a cloud-hosted control plane; replacing the editor/terminal; silent autonomy; raw-TUI screen-scraping; shipping/operating Microsoft's proprietary server/Marketplace.

## Appendix A — Confirmed platform gaps & required workarounds (part of the contract; re-verify versions at build time)
- **A1 — Claude `Notification` AND `PermissionRequest` hooks broken in the VS Code extension UI.** Only `Stop`, `UserPromptSubmit`, `PreToolUse` fire there (issue #31285, ext v2.1.69; corroborated #59718, #11156, #16114, #29928; #28774 dup; #8320 idle NOT_PLANNED even in CLI). → §8.1 inference + Use-Terminal upgrade.
- **A2 — VS Code integrated terminal drops OSC 9/777** at the renderer (#28338) → recover via the **stable shell-integration read stream** (§8.1), proven by `wenbopan.vscode-terminal-osc-notifier` (engine ^1.93.0, no proposed API).
- **A2-codex — Codex external `notify` fires only on `agent-turn-complete`** (#19921 vs 0.125.0); approval/plan-mode only via `[tui] notifications`/app-server (#17417 merged for TUI). → §8.2 app-server primary.
- **A3 — `onDidWriteTerminalData` is still proposed**, not publishable on the Marketplace (#78146/#145234/#131165) → never depend on it; use the stable read stream or owned-PTY.
- **A4 — Editor windows are mutually isolated** (one extension host/window, no cross-window IPC) → external Hub mandatory (§6.1).
- **A5 — Cross-OS foreign-window focus:** macOS/Windows/X11 workable (Windows needs the SendInput-ALT trick); **Wayland cannot guarantee it** → §18.2 fallback.
- **A6 — Cursor/Windsurf remote diverges** (Open-VSX `open-remote-ssh`; MS restriction April 2025) → §12.3; "editor is the OS" canonical only on MS VS Code.
- **A7 — Microsoft VS Code Server + Marketplace licensing** forbids providing the software as an integrated offering for others to use → deployed environments use code-server/openvscode-server + Open-VSX (§13.1, §19.1).
- **A8 — VS Code/MS dev-tunnels are MS-operated with limits** and not a sanctioned third-party data plane → depend on the *network reachability* the editor connection provides, implemented as the reporter's own outbound WebSocket (§14.2).

## Appendix B — Prior art / competitive landscape (monitor)
- **VS Code "Agents window" (Stable preview, v1.120/1.121, May 2026)** — the closest first-party competitor: aggregates Copilot CLI/Cloud + Claude agent sessions across workspaces in one sidebar with status incl. "waiting for input," and (v1.121) remote agent sessions over SSH/dev-tunnels that survive disconnect. **But:** tied to a Copilot subscription, VS Code-only, and gets state via the providers' SDK/harness — it does **not** aggregate arbitrary self-launched terminal Claude/Codex CLI sessions, and not Cursor/Windsurf. That non-coverage is Fleet's differentiation. **Track every release.**
- **Claude Code "Agent View"** (research preview, v2.1.139+) — Claude-only, CLI-local; not cross-tool/cross-environment.
- **cmux** (AGPL, macOS-only, Ghostty-based) — owns the terminal, so OSC detection is easy for it; Unix-socket scriptable API; no VS Code integration, no documented phone-home/subscribe API. Design inspiration for the ring/inbox UX, not a base.
- **Coder / Gitpod / Daytona** — workspace provisioning + Hetzner templates to reuse for deploy (§13.3); not attention-inbox tools.
- **Detection/notify plumbing to reuse:** `wenbopan.vscode-terminal-osc-notifier` (read-stream OSC recovery), JSONL-tailing status tools (KyleJamesWalker/vscode-cc-agent-manager, patoles/agent-flow), Simon Alford's hooks→OSC setup.

## Appendix C — Key references
Claude Code hooks: code.claude.com/docs/en/hooks; issues anthropics/claude-code #8320, #8985, #11156, #16114, #28338, #28774, #29928, #31285, #59718.
Codex: developers.openai.com/codex/app-server, /config-advanced, /hooks; issues openai/codex #2109, #13478, #17417 (merged), #18385, #18391, #19921.
VS Code: code.visualstudio.com/docs/terminal/shell-integration; /api/references/vscode-api; v1.93 release notes (shell integration API stable); issues microsoft/vscode #78146, #131165, #145234, #121926; Agents window docs (/docs/copilot/agents/agents-window).
Remote/licensing: code.visualstudio.com/docs/remote/faq, /preview-license; Open-VSX; jeanp413/open-remote-ssh.
Host/transport: Tauri 2.x; MQTT persistent-session semantics; cmux socket API; Coder/Daytona Hetzner templates.

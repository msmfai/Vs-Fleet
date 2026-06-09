# 19 — Agent in env (claude lifecycle → Hub)

The agent-observability spine: a real `claude` running **inside** one fleet-env
container fires hooks → the container's `claude` shell wrapper → the linux
`fleet-reporter` (S15 adapter for working/idle/done, S16 infer adapter for the
inferred `waiting`) → phones home over WS to the **host-side Hub** (`:51777`) → the
session is registered under a title == `FLEET_SERVER_ID` == `env.id` → asserted by
the harness via the `fleet ls --once` CLI on the host.

**Assertion mechanism (the only one this area uses for Hub state).** `fleet ls --once`
prints one row per session:

```
[<state>]<unread> <title>  (<N> run[s])  [<urgency>]
```

- `<state>` ∈ `{working, waiting, idle, done, error, dead}` (rollup state; precedence
  `waiting > error > working > done > idle > dead`, see `fleet-cli/src/render.rs`).
- `<urgency>` label ∈ `  [approval]` | `  [question]` | `""` (precedence
  `approval > question > none`).
- `<unread>` = ` *` when the session has an unread notification.
- `(N runs)` = count of `AgentRun`s on the session.

The harness helpers in `behaviours/agentInput.mjs` codify this: `hubSnapshot()` runs
`fleet ls --once`, `sessionLineFor(snap, env.id)` matches the row, `stateOf(line)`
extracts `[<state>]`, and `pollHub(title, match, {ms,every})` polls until `match`.
Every entry below asserts on those fields. The reporter `--serve` Unix socket inside
the env is `/tmp/fleet-reporter.sock`; controlled hook frames are injected with
`printf 'claude <json>\n' | nc -N -U /tmp/fleet-reporter.sock` (the `claude ` tag
selects the S15+S16 Claude adapters; an untagged line is treated as a legacy manual
Claude payload).

**Two runtime gates that MUST skip cleanly, never hard-fail** (environmental, not
regressions): (1) container claude unauthenticated (no `ANTHROPIC_API_KEY`, no host
Keychain OAuth → `env.claudeAuthed === false`); (2) Hub/`fleet` CLI absent or the
session never registered. Behaviours that drive a *real* claude carry gate (1);
all carry gate (2). Behaviours that only inject frames into the reporter socket
carry only gate (2) (no claude, no auth needed).

---

### L1.AGENT.001 — `claude -p` drives the env's Hub session active→terminated
- layer: L1
- scenarios: [base, agent-auth]
- isolation: fresh
- needs: [termSend]
- precondition: env booted; reporter phoned home; `fleet ls` shows the env's session row `[idle] <env.id>`; `env.claudeAuthed === true`
- action: `termSend {text: 'claude -p "say hi"\n'}` into a terminal (the baked `claude` shell wrapper installs the Fleet hooks)
- expected: the Hub session for `env.id` goes ACTIVE (`working` or `waiting`) and then TERMINATES (`idle` | `done` | `dead`); ≥1 run recorded
- assert: `pollHub(env.id, st==="working"||st==="waiting", {ms:30000})` ok OR `settled.seen` includes working/waiting; THEN `pollHub(env.id, st==="idle"||st==="done"||st==="dead", {ms:90000})` ok; corroborated by `(N runs)` regex `/\(\d+ runs?\)/` on the final line
- machine-state: procs +1..+4 (node claude + reporter child) during the run, settling back; mem Δ transient
- edges: working window faster than the 750ms poll → accept the `(N runs)` count as proof a run occurred (the working blip may be missed)
- why: guards the entire agent-observability spine: container claude → hook wrapper → S15 reporter state machine → WS phone-home → Hub session registry → CLI render. A break = Fleet is blind to agent activity. Auth/Hub gates SKIP (env, not regression).
- status: implemented (behaviour `agent.claudeRuns`)

### L1.AGENT.002 — `working` is emitted on UserPromptSubmit / first PreToolUse
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: Hub up; session `[idle] <env.id>` registered
- action: inject one controlled frame to `/tmp/fleet-reporter.sock`: `claude {hook_event_name:"UserPromptSubmit", session_id:"w-<env.id>", cwd:"/home/coder/project"}`
- expected: the Hub session for `env.id` transitions `idle → working`
- assert: `pollHub(env.id, st==="working", {ms:15000})` ok; `stateOf(boot.line)==="idle"` before; deterministic (no real claude → no auth gate)
- edges: a second `UserPromptSubmit` for the SAME `session_id` while already `working` is idempotent (stays `working`, no new run minted — `run_counter` only increments on a *new* `session_id`)
- why: pins the S15 mapping `UserPromptSubmit|PreToolUse → working` at the socket→Hub→CLI boundary (Rust unit tests cover the pure machine; this covers the wire). Refactors that drop the working edge silently hide active agents.
- status: TODO

### L1.AGENT.003 — `Stop` settles the run to `idle` (turn finished, awaiting prompt)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session driven to `working` via an injected `UserPromptSubmit` (as L1.AGENT.002)
- action: inject `claude {hook_event_name:"Stop", session_id:"w-<env.id>", cwd:"/home/coder/project", stop_hook_active:false}`
- expected: the Hub session transitions `working → idle` (a bare Stop = turn ended, awaiting next prompt; D9)
- assert: `pollHub(env.id, st==="idle", {ms:15000})` ok after first seeing `working`; final `[idle]`
- edges: `Stop` with `stop_hook_active:true` (a Stop fired from within a Stop hook's own continuation) must NOT settle — state stays `working` (conservative, not a real task end)
- why: `Stop` is THE completion signal; misclassifying it leaves runs stuck `working` forever (false "agent busy") or over-claims `done`. Guards the idle-vs-continuation distinction at the wire.
- status: TODO

### L1.AGENT.004 — `Stop` with a completion marker settles to `done` (not idle)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `working` (injected `UserPromptSubmit`)
- action: inject `claude {hook_event_name:"Stop", session_id:"d-<env.id>", cwd:"/home/coder/project", stop_hook_active:false, reason:"completed"}` (or a build's `subtype:"success"` envelope)
- expected: the Hub session transitions `working → done` (completion marker present)
- assert: `pollHub(env.id, st==="done", {ms:15000})` ok; `[done]` on the row
- edges: a Stop with neither a completion marker NOR `stop_hook_active` → conservatively `idle` (covered by L1.AGENT.003); never over-claim `done`
- why: distinguishes a *finished task* (`done`, dismissible) from *turn-paused-awaiting-prompt* (`idle`); the rollup/urgency UI treats them differently. Pins D9 at the socket boundary.
- status: TODO

### L1.AGENT.005 — Inferred `waiting` on a PreToolUse-without-Stop (S16 debounce)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: Hub up; session `[idle] <env.id>` registered; reporter `--serve` running (its INFER_TICK timer drives the debounce, finer than DEFAULT_DEBOUNCE_MS=1500)
- action: inject ONE `claude {hook_event_name:"PreToolUse", session_id:"wait-<env.id>", tool_name:"Bash", tool_use_id:"toolu_fleetwait", cwd:"/home/coder/project"}` with no follow-up frame
- expected: after one debounce window (≥1.5s) the serve tick fires and the Hub session reaches `waiting`
- assert: `pollHub(env.id, st==="waiting", {ms:45000})` ok; THEN inject a `Stop` (same session_id) to resolve cleanly so the env is left non-blocked
- machine-state: none (no claude process; pure socket injection)
- edges: covered as separate entries — resolve-before-fire (L1.AGENT.006), repeat PreToolUse (L1.AGENT.007), JSONL veto (L1.AGENT.008)
- why: the ONLY end-to-end guard that serve's tick-driven inference + urgency/rollup plumbing + CLI `[waiting]` rendering actually emit the approval-needed signal (Fleet's core ping). We inject a controlled frame rather than a flaky real Bash-approval block (version/permission-mode dependent) — same socket path, deterministic timing. Red ⇒ suspect serve tick or Waiting plumbing, not claude.
- status: implemented (behaviour `agent.waitingState`)

### L1.AGENT.006 — Activity before the debounce cancels the pending `waiting`
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `[idle]`; reporter `--serve` running
- action: inject `PreToolUse` (session `cancel-<env.id>`), then within <1.5s inject a `Stop` (same session) BEFORE the debounce tick fires
- expected: the Hub session NEVER shows `waiting` — the follow-up cancels the armed inference; it settles to `idle`
- assert: `pollHub(env.id, st==="waiting", {ms:4000})` returns NOT ok (no waiting seen); a subsequent `pollHub(env.id, st==="idle", {ms:8000})` ok
- why: the debounce must not fire when the tool was approved/ran quickly; a false `waiting` would ping the user for nothing. Guards "any later activity cancels" at the wire (the inverse of L1.AGENT.005).
- status: TODO

### L1.AGENT.007 — Repeat PreToolUse re-arms the debounce, single waiting raised
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `[idle]`; reporter `--serve` running
- action: inject `PreToolUse` (session `rearm-<env.id>`), let it reach `waiting`, then inject a SECOND `PreToolUse` (same session) — a new tool dispatch while still blocked
- expected: still exactly one `waiting` on the session (the second PreToolUse re-arms, does not stack a second waiting run); session stays `[waiting]`
- assert: after both frames `pollHub(env.id, st==="waiting", {ms:6000})` ok; `(N runs)` count does NOT increase beyond 1 for this session (regex on the row); resolve with a `Stop`
- why: real agents fire multiple PreToolUse on one blocked turn; the inference must coalesce, not multiply the ping. Guards against waiting-run duplication under repeated tool dispatch.
- status: TODO

### L1.AGENT.008 — JSONL transcript `Resolved` vetoes the inferred waiting
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: a transcript JSONL exists in the env whose LAST `tool_use` has a matching `tool_result` (write it via `writeFile` to a `transcript_path`); reporter `--serve` running
- action: inject `PreToolUse {session_id:"veto-<env.id>", tool_use_id:"toolu_done", transcript_path:"<that path>"}` then wait past the debounce
- expected: NO `waiting` raised — `corroborate_jsonl` returns `Resolved` (tool already completed), which vetoes the debounce
- assert: `pollHub(env.id, st==="waiting", {ms:5000})` NOT ok; session remains `[idle]`/`[working]`
- edges: an unreadable/drifted transcript → `Corroboration::Unknown` → decides on timing alone → `waiting` IS raised (Unknown must never suppress a genuine approval) — assert that variant raises `waiting`
- why: corroboration must *veto* a false positive when the transcript proves the tool ran, but `Unknown` must never suppress a real block. Guards the schema-drift-guarded JSONL corroboration at the socket→Hub path.
- status: TODO

### L1.AGENT.009 — Real claude into a Bash approval block surfaces `waiting`
- layer: L1
- scenarios: [agent-auth]
- isolation: fresh
- needs: [termSend]
- precondition: `env.claudeAuthed === true`; Hub up; session registered; default permission mode (Bash gated on approval)
- action: `termSend` an INTERACTIVE `claude` (NOT `-p`) with a prompt forcing a Bash tool, e.g. `claude\n` then the prompt `run "echo hi" in bash\n`; claude fires PreToolUse(Bash) then blocks on y/n with no Stop
- expected: the Hub session reaches `waiting` via the S16 inference of the REAL hook stream
- assert: `pollHub(env.id, st==="waiting", {ms:45000})` ok; THEN ALWAYS unblock (`termSend ""` Ctrl-C + `exec pkill -f claude`) so the env never hangs
- edges: if the headless block never reaches `working` (claude version/permission default changed so it auto-approves) → SKIP with a precise reason, NOT a hard fail
- why: the realism counterpart to L1.AGENT.005 — proves a genuine native-UI approval block (where PermissionRequest/Notification hooks do NOT fire, #issue) is inferred. Flaky by nature → bounded + always-unblock + skip-on-no-block; the deterministic guard is L1.AGENT.005.
- status: TODO

### L1.AGENT.010 — `SessionEnd` marks the run `dead` (one-shot `-p` exit)
- layer: L1
- scenarios: [base, agent-auth]
- isolation: fresh
- needs: []
- precondition: session `working` (injected `UserPromptSubmit` for `end-<env.id>`)
- action: inject `claude {hook_event_name:"SessionEnd", session_id:"end-<env.id>", cwd:"/home/coder/project"}`
- expected: the Hub session transitions to `dead` (the session closed)
- assert: `pollHub(env.id, st==="dead", {ms:15000})` ok; `[dead]` on the row (and per render sorting, dead sorts last)
- edges: `SessionEnd` for an UNKNOWN `session_id` (never started) → no-op, no row mutation (assert the row's state is unchanged)
- why: `-p` is one-shot — it fires SessionEnd on exit, so `agent.claudeRuns` accepts `dead` as a legitimate terminal state. Pins the `SessionEnd → dead` mapping; without it a finished one-shot run would look stuck.
- status: implemented (behaviour `agent.claudeRuns` accepts `dead` as a terminal state)

### L1.AGENT.011 — Multi-turn: same session_id keeps ONE run, cycles working↔idle
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `[idle]`
- action: inject the sequence for ONE `session_id="mt-<env.id>"`: UserPromptSubmit → Stop → UserPromptSubmit → Stop (two turns)
- expected: the session stays a SINGLE run (`(1 run)`) that cycles `idle→working→idle→working→idle`; no second run minted (run minted per *new* session_id, not per turn)
- assert: at end `pollHub(env.id, st==="idle")` ok; `(1 run)` via regex on the row; `pollHub` `seen` array shows the working↔idle cycle
- edges: a DIFFERENT `session_id` on the second turn (a `--resume` that re-anchors) → covered separately in L1.AGENT.012
- why: durable identity (D4) — the Claude `session_id` is the run's `native_id`; multi-turn must not spawn phantom runs. Guards run-count stability across an interactive multi-turn conversation.
- status: TODO

### L1.AGENT.012 — `--continue`/`--resume` keeps the same session_id / native_id
- layer: L1
- scenarios: [agent-auth]
- isolation: fresh
- needs: [termSend]
- precondition: `env.claudeAuthed`; a first `claude -p "say hi"` completed (run on session S1)
- action: `termSend 'claude --continue -p "and bye"\n'`
- expected: the resumed turn reports under the SAME Claude `session_id` (stable across `--continue`/`--resume`, S6) → the SAME Hub run (`native_id` unchanged), run count does NOT increase
- assert: capture the session_id/run line before and after; `(N runs)` count unchanged across the resume; the session goes working→idle again on the same row
- edges: if `--continue` finds no prior session (fresh env) → claude errors / starts new; SKIP cleanly with reason
- why: `session_id` is validated stable across resume — Fleet anchors `native_id` to it with no broker. A regression that re-derives identity per invocation would split one conversation into many phantom sessions. Guards durable identity end-to-end.
- status: TODO

### L1.AGENT.013 — Tool use: PreToolUse carries tool_name + tool_use_id through to the run
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `[idle]`
- action: inject `UserPromptSubmit` then `PreToolUse {session_id:"tool-<env.id>", tool_name:"Bash", tool_use_id:"toolu_abc123", cwd:...}` then `Stop`
- expected: state goes working (the PreToolUse keeps it working + liveness ping); the run records the tool activity; settles idle on Stop
- assert: `pollHub(env.id, st==="working")` ok after the PreToolUse; final `[idle]`; (where the Hub/CLI exposes run extras) the run's `tool_name`/`tool_use_id` are preserved — else assert the working-liveness edge only
- edges: a `PostToolUse` injected between PreToolUse and Stop is treated as a pure liveness ping and NEVER flips state (it does not fire in native UI, #31285, so it must never be load-bearing) — assert the working→idle path is identical with or without the PostToolUse
- why: pins the S15 contract that PreToolUse = working + liveness while PostToolUse is non-authoritative. A refactor relying on PostToolUse for completion would make `done` silently wrong in the native UI.
- status: TODO

### L1.AGENT.014 — SubagentStop is liveness only, does not end the main run
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: session `working` (injected UserPromptSubmit for `sub-<env.id>`)
- action: inject `claude {hook_event_name:"SubagentStop", session_id:"sub-<env.id>", cwd:...}`
- expected: the main run stays `working` (a subagent's turn finishing does not end the parent turn); only a real `Stop` settles it
- assert: after SubagentStop `pollHub(env.id, st==="working", {ms:4000})` still ok (not idle); a following `Stop` then settles `idle`
- why: a SubagentStop must not be mistaken for the main turn's Stop, or every Task/subagent call would falsely show the parent as finished. Guards the SubagentStop-is-not-completion rule.
- status: TODO

### L1.AGENT.015 — Session correlation: row title == FLEET_SERVER_ID == env.id
- layer: L1
- scenarios: [base, agent-auth]
- isolation: fresh
- needs: []
- precondition: env booted with `-e FLEET_SERVER_ID=<env.id>`; reporter phoned home
- action: read `fleet ls --once` on boot (no agent action)
- expected: exactly one Hub session row whose title contains `env.id`; the reporter registered it at boot (before any agent run), state `[idle]`
- assert: `sessionLineFor(hubSnapshot(), env.id)` returns a row; `stateOf(line)==="idle"`; this is the `boot = pollHub(env.id, ()=>true)` gate every other AGENT entry relies on
- edges: two parallel envs with distinct ids → two distinct rows, each matched by its own id, no cross-talk (assert each env's poll matches only its own row)
- why: correlation is the join key for the whole area — if the reporter mis-titles the session (wrong/missing FLEET_SERVER_ID) every assertion above silently matches the wrong row or nothing. Guards the session-naming contract at boot.
- status: implemented (boot gate in `agent.claudeRuns` / `agent.waitingState` — `pollHub(env.id, ()=>true)`)

### L1.AGENT.016 — Auth gate: unauthenticated container SKIPs, never fails
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend]
- precondition: `env.claudeAuthed === false` (no `ANTHROPIC_API_KEY`, `FLEET_CLAUDE_OAUTH=0` or no Keychain item)
- action: attempt `agent.claudeRuns`
- expected: the behaviour returns `skipped:"container claude not authenticated …"`, `pass:false`, with a precise reason — NOT a hard failure
- assert: result has a non-empty `skipped` field; the reporter counts it as skipped (console/JUnit/HTML/summary branch on `skipped`)
- edges: `ANTHROPIC_API_KEY` present → no skip, runs for real (the positive path is L1.AGENT.001)
- why: auth is environmental, not a regression — turning it into a failure would make the suite red on any machine without credentials. Guards the clean-skip contract so CI without keys stays green.
- status: implemented (behaviour `agent.claudeRuns` auth gate)

### L1.AGENT.017 — Hub-absent gate: missing CLI / unregistered session SKIPs cleanly
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: no `fleet` CLI on PATH / `target/debug/fleet` absent, OR the Hub is down so the session never registers
- action: attempt any AGENT behaviour
- expected: `skipped:"Hub \`fleet\` CLI not found …"` (no CLI) OR `skipped:"Hub session … not found …"` (CLI present but boot poll fails); `pass:false`, never a hang
- assert: `fleetCli()` returns null → skip; else `boot = pollHub(env.id, ()=>true, {ms:15000})` not ok → skip with the session-not-found reason; bounded wait (≤15s), no infinite poll
- edges: CLI present but `fleet ls --once` errors/times out → `hubSnapshot()` catches and returns null → treated as unavailable → skip (not crash)
- why: the Hub lives on the host and is optional in a bare env run; a missing/unreachable Hub must skip with a bounded wait, never hang the harness (the §8 "verify Hub listening before polling, else it hangs" gotcha). Guards the Hub-availability gate + the hang trap.
- status: implemented (behaviour `agent.claudeRuns` / `agent.waitingState` Hub gates)

### L1.AGENT.018 — `waiting` carries `[approval]` urgency in the rollup
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: a `waiting` raised via the S16 PreToolUse-without-Stop path (as L1.AGENT.005)
- action: read `fleet ls --once` once `waiting` is observed
- expected: the session row shows BOTH `[waiting]` state AND the `  [approval]` urgency label (S16 emits `waiting + approval`)
- assert: `stateOf(line)==="waiting"` AND `line.includes("[approval]")`; resolve with a Stop after
- edges: a `question`-urgency waiting (if a future adapter emits it) sorts AFTER approval — assert label is `[approval]` here, not `[question]`
- why: the urgency label is what drives the rail badge/ping priority (approval > question). A waiting with no/ wrong urgency would mis-prioritise the user's attention. Guards the urgency stamping through rollup → CLI render.
- status: TODO

### L1.AGENT.019 — Confidence honesty: inferred waiting is never High-confidence
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: a `waiting` raised via S16 inference (L1.AGENT.005)
- action: inspect the run's confidence (via a Hub/CLI detail query, or the reporter's emitted command if exposed)
- expected: the raised `waiting` carries `Confidence::Inferred`, NEVER `Confidence::High` (only an authoritative `SessionEnd` exit is High in S15/S16)
- assert: the run detail for the waiting shows confidence == inferred; the only High in this adapter is on `SessionEnd`
- edges: the JSONL `Stuck` corroboration raises *quality* but NOT confidence — assert confidence stays `inferred` even with a corroborating transcript
- why: invariant 5 (confidence honesty) is structural — an inferred approval must not masquerade as authoritative, or downstream consumers over-trust a heuristic. Guards that the wire/Hub never up-ranks an inference.
- status: TODO

### L1.AGENT.020 — Concurrent agents in one reporter: per-session multiplexing
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: one env, one reporter `--serve` socket; Hub up
- action: inject interleaved frames for TWO distinct `session_id`s (`a-<env.id>`, `b-<env.id>`): UserPromptSubmit(a), UserPromptSubmit(b), Stop(a), Stop(b)
- expected: the reporter owns one `ClaudeStateMachine` per session_id; both runs appear under the env's session row, each transitioning independently; final `(2 runs)`
- assert: after the sequence the row shows `(2 runs)`; the rollup state reflects the most-urgent across them (per rollup precedence); each run's working→idle is independent (no cross-bleed of state)
- edges: a frame with NO `session_id` (`MissingSessionId` parse error) is rejected — assert no phantom run is minted (identity honesty: no durable anchor → no run)
- why: one reporter shell can host several Claude sessions (S15 `ClaudeReporter` multiplexes per session_id); state must not leak between them, and a missing anchor must be rejected. Guards multi-session correctness + identity honesty at the socket.
- status: TODO

### L1.AGENT.021 — Preexisting long-running agent: session is non-empty before any behaviour
- layer: L1
- scenarios: [preexisting-agent]
- isolation: fresh
- needs: []
- precondition: scenario `setup` started a long `claude` run BEFORE behaviours (state non-empty); `env.claudeAuthed`
- action: read `fleet ls --once` at behaviour start
- expected: the env's session row already shows an ACTIVE state (`working`/`waiting`) and `(≥1 run)` — the suite observes a mid-flight agent, not a cold start
- assert: `sessionLineFor` row exists; `stateOf` ∈ {working, waiting}; `(N runs)` ≥ 1 at observe time
- edges: if the preexisting run already finished by observe time → state may be `idle`/`done`; assert `(≥1 run)` regardless (a run definitely happened)
- why: most real envs are observed mid-flight, not from a clean idle. Guards that the reporter→Hub pipeline reports a run that started before the harness attached (no cold-start assumption baked into the assertions).
- status: TODO

### L1.AGENT.022 — Reset clears prior session state (fresh isolation between runs)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: a prior behaviour drove the env's session to a terminal state on the Hub
- action: a `fresh`-isolation behaviour resets the env (new container, new `FLEET_SERVER_ID`)
- expected: the new env registers a DISTINCT session row (new id) at `[idle]`; the prior env's row does not bleed into the new one's assertions
- assert: the new `env.id` matches a fresh `[idle]` row; `boot.line` state == idle; no `(N runs)` carried over (count starts at 0/1)
- edges: an env that crashed mid-run (no SessionEnd) → its old row may linger as `working`/`dead` on the Hub until reap (L2.HUB TTL); assert the NEW env's row is matched by its own id and is independent
- why: `fresh` isolation must give a clean session per env or cross-behaviour state would corrupt assertions. Guards the reset→new-session contract (the join key changes per fresh env, see L1.AGENT.015).
- status: TODO

### L1.AGENT.023 — Empty state: no agent run leaves the session quiescent `[idle]`
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: []
- precondition: env booted, reporter phoned home, NO agent ever invoked
- action: read `fleet ls --once` (no action)
- expected: the env's session sits at `[idle]` with `(0 runs)` (or no run sub-rows) — registered but quiescent; no spurious `working`/`waiting`
- assert: `stateOf(line)==="idle"`; the row shows no active run / `(0 runs)`; `pollHub(env.id, st==="working"||st==="waiting", {ms:3000})` NOT ok
- why: the empty/idle baseline must be clean — a reporter that fabricates activity on boot would false-ping the user. Guards the quiescent baseline that every active-state assertion is measured against.
- status: TODO

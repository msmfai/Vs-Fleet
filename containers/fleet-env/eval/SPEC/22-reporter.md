# 22 — Reporter (frame parse, adapters, correlation, S16 inference, --serve socket)

L2 area. The reporter (`fleet-reporter --serve`) is the half of Fleet that turns a
container agent's hook payloads into Hub deltas. It binds a unix socket
(`FLEET_REPORTER_SOCKET`, `/tmp/fleet-reporter.sock` in the env), reads one framed
line per hook payload, runs each through `parse_frame` → an agent adapter
(`ClaudeAdapter` S15 / `ClaudeInferAdapter` S16 / `CodexAdapter`), and forwards the
resulting `ReporterCommand`s (`UpsertRun` / `Liveness`) to the Hub over WS. A periodic
TICK (250 ms, `INFER_TICK_INTERVAL`) drives the S16 debounce forward when no hook
arrives. The Hub-facing observable is `fleet ls --once` (a session row
`[<state>]<title> (<n> run[s])[ <urgency>]`); the socket-facing observable is what the
reporter accepts/drops.

Code under test:
- `crates/fleet-reporter/src/serve.rs` — `parse_frame`, `Agent::from_tag`, `Receiver`
  (`process`/`process_at`/`tick`/`tick_at`/`dispatch`), `serve_unix`, `bind_reclaiming`,
  `restrict_socket_perms`, `INFER_TICK_INTERVAL`, `DriftError`.
- `crates/fleet-reporter/src/claude.rs` — S15 `ClaudeHookKind`/`ClaudeHookEvent::parse`,
  `ClaudeStateMachine`, `ClaudeAdapter`.
- `crates/fleet-reporter/src/claude_infer.rs` — S16 `ClaudeInferMachine`/`Adapter`,
  `DEFAULT_DEBOUNCE_MS=1500`, `corroborate_jsonl`/`_for`/`_blob`, `Corroboration`.
- `crates/fleet-reporter/src/codex.rs` — `CodexHookEvent`, `CodexStateMachine`/`Adapter`,
  `ApprovalDecision`, `RawDecision`.
- `crates/fleet-host/src/spawn.rs` — `claude_hooks_settings` (the exact relay frame),
  `install_claude_shim`, `spawn_reporter`.

Wire-frame anchors (verbatim, from `claude_hooks_settings`):
the relay command is `printf 'claude %s\n' "$(cat | tr -d '\r\n')" | nc -U '<sock>'
2>/dev/null || true` — i.e. **tagged `claude `, CR/LF stripped, one line, `|| true`**.

Most entries here exercise the Rust pipeline directly (the in-env behaviour that
matches is `agent.waitingState`, which injects a controlled `PreToolUse`-without-`Stop`
frame straight at the real socket). Entries whose `assert` is a pure-Rust state-machine
property are marked `status: implemented (<crate test>)` only when an exact Rust unit
test already covers them; the L2 socket→Hub→CLI end-to-end is mostly `TODO` / `partial`.

---

## Frame parsing (`parse_frame`, `Agent::from_tag`, `DriftError`)

### L2.RPT.001 — A `claude `-tagged frame parses to (Claude, body)
- layer: L2
- scenarios: [base]
- isolation: shared
- precondition: a `Receiver` (or `parse_frame` directly); line = `claude {"hook_event_name":"Stop","session_id":"s1","cwd":"/repo","stop_hook_active":false}`
- action: `parse_frame(line)`
- expected: `Ok((Agent::Claude, body))` where body starts with `{` and contains `"Stop"`
- assert: returned tuple `.0 == Agent::Claude`; `.1.starts_with('{')` && `.1.contains("Stop")`
- why: the leading agent tag is the only disambiguator (claude/codex payloads overlap on `hook_event_name`+`session_id`); a tagged frame must route to the Claude adapter and hand the *body* (not the whole line) downstream.
- status: implemented (serve test `parse_frame_tagged_claude`)

### L2.RPT.002 — A `codex `-tagged frame parses to (Codex, body); tag is case-insensitive
- layer: L2
- scenarios: [base]
- precondition: lines `codex {…}`, `Codex {}`, `CLAUDE {}`
- action: `parse_frame` each; `Agent::from_tag` for `claude-code`/`claudecode`
- expected: `Codex {…}` → `(Codex, …)`; `Codex {}`/`CLAUDE {}` route by lowercased tag; `claude-code`/`claudecode` → `Some(Agent::Claude)`
- assert: `parse_frame("Codex {}").unwrap().0 == Agent::Codex`; `parse_frame("CLAUDE {}").unwrap().0 == Agent::Claude`; `Agent::from_tag("claude-code") == Some(Agent::Claude)`
- why: `Agent::from_tag` lowercases the token; the sender may emit any case/alias, and a mis-route would feed a codex payload to the claude machine (or vice-versa).
- status: implemented (serve test `parse_frame_tag_is_case_insensitive`)

### L2.RPT.003 — An untagged JSON line defaults to the Claude path
- layer: L2
- scenarios: [base]
- precondition: line = a bare `{"hook_event_name":"Stop","session_id":"s1",…}` with NO leading agent token
- action: `parse_frame(line)`
- expected: `Ok((Agent::Claude, line))` — the whole line is the body
- assert: tuple `.0 == Agent::Claude`; `.1.contains("Stop")`; `.1 == line.trim()`
- why: hand-sent / legacy `printf '{...}' | nc -U` payloads (no tag) must still reach the validated hooks-first Claude path; the untagged branch is a load-bearing compat affordance.
- status: implemented (serve test `parse_frame_untagged_defaults_to_claude`)

### L2.RPT.004 — EDGE: empty / whitespace-only frame is `DriftError::Empty`, dropped, counted
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; lines `""` and `"   "`
- action: `parse_frame("   ")`; then `Receiver::process("   ")`
- expected: `Err(DriftError::Empty)`; through `process`, zero commands AND `frames_dropped()` increments
- assert: `parse_frame("   ") == Err(DriftError::Empty)` && `parse_frame("") == Err(DriftError::Empty)`; `rx.process("   ").is_empty()`; `rx.frames_dropped() == 1`
- why: the drift guard must collapse a malformed frame to ∅ commands + a `debug!` line, never panic and never fabricate state (invariant 2); the dropped counter is the observability hook.
- status: implemented (serve tests `parse_frame_empty_is_drift`, `process_garbage_json_is_dropped_not_panicked`)

### L2.RPT.005 — EDGE: a bodyless tag (`claude` / `claude   `) falls through to untagged-Claude, yields 0 commands
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; lines `"claude   "`, `"codex"`
- action: `parse_frame`, then `Receiver::process`
- expected: `parse_frame("claude   ") == Ok((Agent::Claude, "claude"))` (trailing space trimmed first → single bare token → untagged branch); `process("claude")` and `process("codex")` both return `[]`
- assert: `parse_frame("claude   ") == Ok((Agent::Claude, "claude"))`; `parse_frame("codex") == Ok((Agent::Claude, "codex"))`; `rx.process("claude").is_empty()` && `rx.process("codex").is_empty()`
- why: a tag with no JSON body is not a usable frame; it must degrade to a non-JSON Claude body that the adapter silently drops — never a panic, never a state.
- status: implemented (serve test `parse_frame_bodyless_tag_falls_through_harmlessly`)

### L2.RPT.006 — EDGE: an unknown agent tag is treated as untagged-Claude body (whole line)
- layer: L2
- scenarios: [base]
- precondition: line = `gemini {"hook_event_name":"Stop","session_id":"s1"}`
- action: `parse_frame(line)`
- expected: `Ok((Agent::Claude, line))` — `Agent::from_tag("gemini") == None`, so the whole line (incl. the `gemini` token) becomes the body; the Claude adapter then drops it (leading `gemini` makes it non-JSON)
- assert: `parse_frame(line).unwrap() == (Agent::Claude, line.trim())`; `Receiver::process(line).is_empty()`
- why: an unrecognised sender tag must not crash routing nor mis-claim Claude state; the non-JSON body is harmlessly dropped by the adapter. (Currently no dedicated Rust test asserts the unknown-tag case explicitly.)
- status: partial (no explicit unknown-tag test; covered indirectly by the untagged/bodyless tests)

### L2.RPT.007 — EDGE: tagged-but-garbage body is dropped (no panic, no command), not frame-drift
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; line = `claude this-is-not-json`
- action: `Receiver::process(line)`
- expected: `[]` commands; `frames_dropped()` does NOT increment (adapter-level no-op ≠ frame-level drift)
- assert: `rx.process("claude this-is-not-json").is_empty()`; afterwards `rx.frames_dropped() == 0` (only frame-level Empty bumps the drop counter)
- why: `ingest_json` swallows JSON parse errors returning `Vec::new()`; the drift counter tracks only frame-level drift so observability distinguishes "malformed framing" from "adapter chose not to act".
- status: implemented (serve test `process_garbage_json_is_dropped_not_panicked`)

---

## Claude S15 hook → state map (`ClaudeAdapter` / `ClaudeStateMachine`)

### L2.RPT.010 — UserPromptSubmit → Working (Inferred), native_id == session_id
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; frame = `claude {"hook_event_name":"UserPromptSubmit","session_id":"sess-A","cwd":"/repo"}`
- action: `Receiver::process(frame)`
- expected: one `UpsertRun(run)` with `run.state == Working`, `run.native_id == "sess-A"`, `run.confidence == Inferred`, `run.agent_kind == ClaudeCode`
- assert: find the `ReporterCommand::UpsertRun` in the result; `run.state == State::Working`; `run.native_id == "sess-A"`; `run.confidence == Confidence::Inferred`
- why: a prompt is the canonical activity signal; the durable anchor MUST be the verbatim claude `session_id` (stable across `--continue`/`--resume`, D4), and working is heuristic so it's Inferred not High (invariant 5).
- status: implemented (serve test `process_claude_prompt_then_stop_drives_working_then_idle`)

### L2.RPT.011 — Stop (real turn boundary, stop_hook_active:false) → Idle
- layer: L2
- scenarios: [base]
- precondition: a `Receiver` whose `sess-A` is Working; frame = `claude {"hook_event_name":"Stop","session_id":"sess-A","cwd":"/repo","stop_hook_active":false}`
- action: `Receiver::process(stop_frame)`
- expected: one `UpsertRun(run)` with `run.state == Idle`
- assert: `UpsertRun.state == State::Idle`; `rx.frames_seen() == 2`, `rx.frames_dropped() == 0`
- why: `Stop` is the **the** completion signal for the native UI; absent a completion marker it's conservatively Idle (D9), never over-claiming Done.
- status: implemented (serve test `process_claude_prompt_then_stop_drives_working_then_idle`; end-to-end socket→Hub also via behaviour `reporter.promptWorkingThenStopIdle`)

### L2.RPT.012 — PreToolUse → Working + liveness; last_message == "Running <tool>…"
- layer: L1/L2
- scenarios: [base]
- precondition: fresh `ClaudeAdapter`; event PreToolUse, session `sess-A`, tool_name `Bash`
- action: `ClaudeAdapter::ingest(&ev)` (or `ingest_json`)
- expected: from idle → `UpsertRun(Working)` with `last_message == "Running Bash…"`; a *repeat* PreToolUse while already Working emits `Liveness{run_id}` not UpsertRun (changed==false, liveness==true)
- assert: first ingest → `UpsertRun` with `state==Working` and `last_message==Some("Running Bash…")`; second identical PreToolUse → exactly one `ReporterCommand::Liveness` and no `UpsertRun`
- why: PreToolUse drives working AND refreshes the liveness window; a working→working repeat must not spam Hub deltas (it's a no-op-with-liveness), guarding delta minimality.
- status: implemented (claude_tests covering PreToolUse working + liveness — confirm exact test id during impl)

### L2.RPT.013 — Stop with explicit completion marker → Done (distinct from Idle, D9)
- layer: L2
- scenarios: [base]
- precondition: Working `sess-A`; frame Stop with `"stop_hook_active":false` AND a marker (`"reason":"completed"` | `"subtype":"success"` | `"taskComplete":true`)
- action: `ClaudeAdapter::ingest_json(stop)`
- expected: `UpsertRun(Done)` (NOT Idle); `last_message` == the `last_assistant_message` preview, falling back to `"Task complete."`
- assert: `UpsertRun.state == State::Done`; with `last_assistant_message:"all set"` → `last_message == Some("all set")`; without it → `Some("Task complete.")`
- why: `turn_complete_done = task_complete || reason∈{completed,done} || subtype∈{success,completed}`, and Done≠Idle must never collapse on the wire (D9); the inbox preview shows what Claude actually said.
- status: implemented (claude_tests done-vs-idle marker cases — confirm test id during impl)

### L2.RPT.014 — EDGE: Stop fired from inside a Stop hook (`stop_hook_active:true`) → Idle, never Done
- layer: L2
- scenarios: [base]
- precondition: Working `sess-A`; frame Stop with `"stop_hook_active":true` AND `"reason":"completed"`
- action: `ClaudeAdapter::ingest_json(stop)`
- expected: `UpsertRun(Idle)` — the continuation flag suppresses the Done claim
- assert: `UpsertRun.state == State::Idle` despite the completion marker present
- why: `stop_hook_active==true` denotes a Stop fired from within a Stop hook's own continuation (not a real task end); treating it as Done would over-claim completion.
- status: implemented (claude_tests stop_hook_active suppresses done — confirm test id)

### L2.RPT.015 — SessionEnd → Dead with Confidence::High (the only authoritative S15 High)
- layer: L2
- scenarios: [base]
- precondition: live `sess-A`; frame `claude {"hook_event_name":"SessionEnd","session_id":"sess-A"}`
- action: `ClaudeAdapter::ingest_json`
- expected: `UpsertRun(Dead)` with `confidence == High`, `last_message == "Session closed."`
- assert: `UpsertRun.state == State::Dead`; `.confidence == Confidence::High`; `.last_message == Some("Session closed.")`
- why: a confirmed exit is the ONLY authoritative S15 signal, so it's the only S15 High; everything else is Inferred (invariant 5 is structural here).
- status: implemented (claude_tests SessionEnd→dead/high — confirm test id)

### L2.RPT.016 — SessionStart on a *dead* session revives it to Idle (resume/continue edge)
- layer: L2
- scenarios: [base]
- precondition: `sess-A` already Dead (SessionEnd delivered); frame `SessionStart` for `sess-A`
- action: `ClaudeAdapter::ingest_json(session_start)`
- expected: `UpsertRun(Idle)` (changed==true via `into_changed`); a SessionStart on an *already-live* session is a no-op (no command)
- assert: dead→start yields `UpsertRun.state == State::Idle`; live→start yields `[]`
- why: `--resume`/`--continue` reuse the same `session_id`; a SessionStart must revive a reaped session but must NOT spuriously reset a live one.
- status: implemented (claude_tests session resume edge — confirm test id)

### L2.RPT.017 — EDGE: PostToolUse and SubagentStop are liveness-only, never flip state (#31285)
- layer: L2
- scenarios: [base]
- precondition: Working `sess-A`; frames PostToolUse then SubagentStop for `sess-A`
- action: `ClaudeAdapter::ingest_json` each
- expected: each emits `Liveness{run_id}` only; state stays Working (no `UpsertRun`, no Idle/Done)
- assert: each result == exactly one `ReporterCommand::Liveness` and zero `UpsertRun`; `adapter.state_of("sess-A") == Some(State::Working)`
- why: PostToolUse does NOT fire in the native UI (anthropics/claude-code #31285), so done is derived from Stop only; SubagentStop ends a *subagent*, not the main run — neither may flip the run.
- status: implemented (claude_tests PostToolUse/SubagentStop liveness-only — confirm test id)

### L2.RPT.018 — EDGE: unknown hook name parses to `Other(_)`, yields no command (schema-drift tolerance)
- layer: L2
- scenarios: [base]
- precondition: frame `claude {"hook_event_name":"NewFutureHook","session_id":"sess-A"}`
- action: `ClaudeHookEvent::parse` then `ClaudeAdapter::ingest`
- expected: parses to `ClaudeHookKind::Other("NewFutureHook")`; `apply` is a `no_op(false)` → no command
- assert: `parse(...).unwrap().kind == ClaudeHookKind::Other("NewFutureHook".into())`; `ingest` returns `[]`
- why: a future Claude build adding a hook must never panic the parser nor fabricate a transition (invariant 2); unknown names are preserved, not rejected.
- status: implemented (claude_tests Other-hook tolerance — confirm test id)

### L2.RPT.019 — EDGE: missing session_id / missing hook_event_name → parse error, dropped
- layer: L2
- scenarios: [base]
- precondition: frames `claude {"hook_event_name":"Stop"}` (no session_id) and `claude {"session_id":"s1"}` (no event name) and `claude {"hook_event_name":"Stop","session_id":""}` (empty id)
- action: `ClaudeHookEvent::parse` each; `ClaudeAdapter::ingest_json` each
- expected: `Err(MissingSessionId)` / `Err(MissingEventName)` / `Err(MissingSessionId)` (empty filtered); `ingest_json` returns `[]` for all
- assert: `parse(no_session).unwrap_err() == ClaudeParseError::MissingSessionId`; `parse(no_event).unwrap_err() == ClaudeParseError::MissingEventName`; empty-id case also `MissingSessionId`; each `ingest_json` is `[]`
- why: the two identity fields are the only hard requirements (no anchor ⇒ no durable run, identity honesty); a frame lacking them is dropped, never given a fabricated id.
- status: implemented (claude_tests parse-error cases — confirm test id)

### L2.RPT.020 — EDGE: camelCase aliases (`hookEventName`,`sessionId`) parse identically
- layer: L2
- scenarios: [base]
- precondition: frame `claude {"hookEventName":"UserPromptSubmit","sessionId":"sess-A","cwd":"/repo"}`
- action: `ClaudeHookEvent::parse` then `ingest`
- expected: same as the snake_case form — `UpsertRun(Working)`, native_id `sess-A`
- assert: parsed `.kind == UserPromptSubmit` && `.session_id == "sess-A"`; ingest → `UpsertRun(Working)`
- why: some builds emit camelCase; the `#[serde(alias …)]` defensiveness must keep detection working regardless of casing.
- status: implemented (claude_tests alias parse — confirm test id)

### L2.RPT.021 — Distinct sessions get distinct Fleet run-ids; one Receiver multiplexes
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; frames UserPromptSubmit for `sess-A` then `sess-B`
- action: `Receiver::process` each
- expected: each yields an `UpsertRun` with a different `run_id`; run-id shape `claude:<session>:run-<n>`
- assert: `run_id(a) != run_id(b)`; both match `^claude:sess-[AB]:run-\d+$`
- why: one reporter shell hosts several claude invocations; the adapter keys per `session_id` so concurrent sessions never cross-contaminate state or identity.
- status: implemented (serve test `process_two_sessions_get_distinct_runs`)

---

## S16 inferred Waiting (`ClaudeInferMachine` / `ClaudeInferAdapter`, debounce + tick + cancel)

### L2.RPT.030 — PreToolUse arms but does NOT emit Waiting before the debounce elapses
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; frame PreToolUse `sess-W` at `now_ms=0`
- action: `Receiver::process_at(frame, 0)`; then `tick_at(DEFAULT_DEBOUNCE_MS - 1)`
- expected: the process emits S15 `Working` but NO `Waiting`; the pre-window tick is a no-op (`[]`)
- assert: no `UpsertRun` with `state == Waiting` in the process result; `rx.tick_at(window-1).is_empty()`; `infer.is_debouncing()==true`
- why: the inferred ping must only fire once the window elapses with no new input; arming on PreToolUse must not prematurely raise waiting.
- status: implemented (serve test `pretool_then_tick_past_debounce_infers_waiting`)

### L2.RPT.031 — tick at/after DEFAULT_DEBOUNCE_MS fires inferred Waiting (Approval, Inferred)
- layer: L2
- scenarios: [base]
- precondition: armed `sess-W` (PreToolUse at t=0, tool_use_id `toolu_x`)
- action: `Receiver::tick_at(DEFAULT_DEBOUNCE_MS)`
- expected: one `UpsertRun(run)` with `state == Waiting`, `urgency == Approval`, `confidence == Inferred`, `native_id == "sess-W"`, `waiting_since == Some(...)`, `last_message` ~ `"Possibly waiting on Bash (inferred)"`
- assert: tick result `UpsertRun.state == State::Waiting`; `.urgency == Some(Urgency::Approval)`; `.confidence == Confidence::Inferred`; `.native_id == "sess-W"`; `.waiting_since.is_some()`
- why: this is Fleet's core ping — the inferred approval-needed signal — and it must NEVER be High (Claude exposes no authoritative waiting hook in native UI); honesty is structural (invariant 5).
- status: implemented (serve test `pretool_then_tick_past_debounce_infers_waiting`)

### L2.RPT.032 — EDGE: a tick exactly at the boundary fires; once fired it is idempotent
- layer: L2
- scenarios: [base]
- precondition: armed `sess-W` at t=0
- action: `tick_at(window)` then `tick_at(window*5)`
- expected: first tick fires one `UpsertRun(Waiting)`; the second tick is a no-op (already fired, `armed_since` cleared in `fire_inference`)
- assert: first `tick_at(window)` → exactly one `UpsertRun(Waiting)`; second `tick_at(window*5)` → `[]`
- why: the debounce uses `now_ms.saturating_sub(armed_at) >= debounce_ms`, fires once and clears `armed_since`; a stream of ticks must not re-emit Waiting every interval (no ping spam).
- status: partial (boundary fire covered; explicit idempotent-second-tick assertion is in claude_infer_tests, not the serve test — confirm test id)

### L2.RPT.033 — Activity (Stop) before the debounce CANCELS the pending inference
- layer: L2
- scenarios: [base]
- precondition: armed `sess-X` (PreToolUse at t=0); then Stop at t=window/2
- action: `process_at(pretool, 0)`; `process_at(stop, window/2)`; `tick_at(window*5)`
- expected: no `Waiting` ever fires; the Stop drove the S15 run Idle and cleared the arm
- assert: `tick_at(window*5)` has no `UpsertRun` with `state==Waiting`; `infer.is_debouncing()==false`
- why: any later activity (`Stop`/`PreToolUse`/`UserPromptSubmit`/`PostToolUse`) cancels a pending debounce so a tool that *did* run is never mislabelled as blocked.
- status: implemented (serve test `activity_before_debounce_cancels_the_inference`)

### L2.RPT.034 — Activity AFTER an inferred Waiting auto-resolves it back to Working
- layer: L2
- scenarios: [base]
- precondition: `sess-W` already in inferred Waiting (tick fired); then a `UserPromptSubmit`/`PreToolUse` arrives
- action: `process_at(activity, window+t)`
- expected: an `UpsertRun(Working)` with `resolved_inference==true` semantics (state leaves Waiting)
- assert: the activity ingest yields `UpsertRun.state == State::Working`; machine `is_inferred_waiting()==false`
- why: the human answered the approval and the agent resumed; the raised waiting must clear with no Fleet interaction (auto-resolve), mirroring the codex auto-resolve.
- status: implemented (claude_infer_tests auto-resolve-after-waiting — confirm test id)

### L2.RPT.035 — EDGE: PostToolUse after a raised Waiting resolves to Working; PostToolUse while merely armed cancels silently
- layer: L2
- scenarios: [base]
- precondition: (a) `sess-W` in inferred Waiting; (b) `sess-Y` armed but not yet fired
- action: PostToolUse for each
- expected: (a) `UpsertRun(Working)` (`resolved_inference` path, `enter_working`); (b) just clears `armed_since` → `Liveness`/no-op, no UpsertRun
- assert: (a) → `UpsertRun.state==Working`; (b) → no `UpsertRun(Waiting)` and no `UpsertRun` at all (a `Liveness` or empty), `is_debouncing()==false`
- why: PostToolUse means the tool completed — it must both cancel a pending arm and auto-resolve a raised inference, distinguishing the two cases (resolved vs. merely-disarmed).
- status: implemented (claude_infer_tests PostToolUse resolve/cancel — confirm test id)

### L2.RPT.036 — S16 runs in parallel with S15: every Claude frame feeds BOTH adapters
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; a single PreToolUse frame for `sess-A`
- action: `Receiver::process(frame)` (dispatch fans to `claude` then `infer`)
- expected: the result contains the S15 Working `UpsertRun` first; the infer adapter's arm produces no immediate command (the fire is deferred to tick) — so the frame yields the S15 delta plus any infer auto-resolution, never a duplicate Working
- assert: result has exactly one `UpsertRun(Working)` (S15); no second `UpsertRun(Working)` from infer on the same frame (infer arming is changed-but-equal Working → still a delta only if state actually moved). Confirm `cmds` ordering: S15 cmd precedes any infer cmd.
- why: `dispatch` does `cmds = claude.ingest; cmds.extend(infer.ingest)` — the two adapters must agree on the session anchor while minting independent run-ids; the S15 path must lead and never be disturbed by the additive infer path.
- status: implemented (serve test `s15_working_idle_path_stays_intact_and_never_waits`)

### L2.RPT.037 — S15 working/idle path is never perturbed by S16 (never fabricates Waiting)
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; prompt then stop for `sess-Y`
- action: `process(prompt)`; `process(stop)`
- expected: prompt → contains `UpsertRun(Working)`; stop → contains `UpsertRun(Idle)`; neither contains `UpsertRun(Waiting)`
- assert: prompt result has an `UpsertRun(Working)`; stop result has an `UpsertRun(Idle)`; neither has any `UpsertRun(Waiting)`
- why: the additive infer adapter must not regress the proven S15 working/idle path; a UserPromptSubmit/Stop sequence never goes through PreToolUse-debounce, so Waiting must never appear.
- status: implemented (serve test `s15_working_idle_path_stays_intact_and_never_waits`)

### L2.RPT.038 — DEFAULT_DEBOUNCE_MS is 1500 and INFER_TICK_INTERVAL is finer (250 ms)
- layer: L2
- scenarios: [base]
- precondition: the consts in `claude_infer.rs` / `serve.rs`
- action: read `DEFAULT_DEBOUNCE_MS` and `INFER_TICK_INTERVAL`
- expected: `DEFAULT_DEBOUNCE_MS == 1500`; `INFER_TICK_INTERVAL == Duration::from_millis(250)`; tick interval ≪ debounce so the inferred waiting surfaces within ~one window of the real stall
- assert: `DEFAULT_DEBOUNCE_MS == 1500`; `INFER_TICK_INTERVAL == Duration::from_millis(250)`; `INFER_TICK_INTERVAL.as_millis() * 2 <= DEFAULT_DEBOUNCE_MS as u128`
- why: the tick must be comfortably finer than the debounce (so waiting fires within roughly one window) but coarse enough not to spin — a const drift would either delay the ping or burn CPU.
- status: TODO (no const-relationship guard test exists; values are load-bearing and unguarded)

---

## Transcript-JSONL corroboration (`corroborate_jsonl` / `_for` / `_blob`, `Corroboration`)

### L2.RPT.040 — Last tool_use with no matching tool_result → Stuck (corroborates the debounce)
- layer: L2
- scenarios: [base]
- precondition: a JSONL blob whose last `message.content[]` has `{"type":"tool_use","id":"toolu_1"}` and NO `tool_result` for `toolu_1`
- action: `corroborate_jsonl(blob)`
- expected: `Corroboration::Stuck`
- assert: `corroborate_jsonl(blob) == Corroboration::Stuck`
- why: a dispatched-but-uncompleted tool is consistent with "blocked on the user"; Stuck lets the debounce stand (raises *quality*, never confidence above Inferred).
- status: implemented (claude_infer_tests corroborate stuck — confirm test id)

### L2.RPT.041 — A matching tool_result → Resolved, which VETOES the inference (auto-resolves a raised Waiting)
- layer: L2
- scenarios: [base]
- precondition: `sess-W` in inferred Waiting; blob has `tool_use id=toolu_1` AND `tool_result tool_use_id=toolu_1`
- action: `ClaudeInferAdapter::corroborate(sess-W, Resolved)` (or `corroborate_blob`)
- expected: `Corroboration::Resolved`; the adapter emits `UpsertRun(Working)` clearing the raised waiting
- assert: `corroborate_jsonl(blob) == Resolved`; `adapter.corroborate("sess-W", Resolved)` returns an `UpsertRun(Working)`
- why: a completed tool means the agent was NOT blocked; Resolved is the only verdict that vetoes the debounce / clears a raised waiting — preventing a false ping.
- status: implemented (claude_infer_tests corroborate resolved veto — confirm test id)

### L2.RPT.042 — EDGE: unparseable / tool-free transcript → Unknown, never suppresses a genuine waiting
- layer: L2
- scenarios: [base]
- precondition: blobs: malformed JSON lines; a valid blob with no tool blocks; truncated
- action: `corroborate_jsonl(blob)` for each
- expected: `Corroboration::Unknown` for all; folded via `corroborate` it is a no-op (decide on timing alone)
- assert: each → `Corroboration::Unknown`; `machine.corroborate(Unknown)` leaves state unchanged (`changed==false`)
- why: the JSONL schema is community-documented/version-sensitive, so the parser degrades to Unknown behind a schema-drift guard rather than panicking or overstating; Unknown must never veto a real approval.
- status: implemented (claude_infer_tests corroborate unknown degrade — confirm test id)

### L2.RPT.043 — Precise correlation: `corroborate_jsonl_for(tool_use_id)` checks THAT tool, not the last-dispatched
- layer: L2
- scenarios: [base]
- precondition: blob where `toolu_armed` is outstanding but a LATER `toolu_other` has a result; armed PreToolUse carried `tool_use_id=toolu_armed`
- action: `corroborate_jsonl_for(blob, "toolu_armed")`; and `corroborate_blob` which auto-selects the precise id when `armed_tool_use_id` is set
- expected: `_for` → `Stuck` (the armed tool is uncompleted) even though another later tool resolved; `corroborate_blob` picks `_for` because `armed_tool_use_id()==Some("toolu_armed")`
- assert: `corroborate_jsonl_for(blob,"toolu_armed") == Stuck`; with a different last-dispatched tool, plain `corroborate_jsonl` would differ — proving precise correlation matters; `corroborate_blob` routes to `_for`
- why: tools can run in parallel / the transcript can lag; correlating on the exact `tool_use_id` the hook armed on ("did *this* tool get a result?") is correct where last-dispatched is wrong.
- status: implemented (claude_infer_tests precise-correlation `_for`/`_blob` — confirm test id)

### L2.RPT.044 — EDGE: `corroborate_jsonl_for` with an id never seen → Unknown
- layer: L2
- scenarios: [base]
- precondition: blob with tools but none whose id == `toolu_missing`
- action: `corroborate_jsonl_for(blob, "toolu_missing")`
- expected: `Corroboration::Unknown` (id not dispatched ⇒ decide on timing alone)
- assert: `corroborate_jsonl_for(blob,"toolu_missing") == Corroboration::Unknown`
- why: an unseen id must not be read as "resolved" (which would suppress a real ping) nor "stuck"; Unknown defers to timing — never suppress a genuine approval.
- status: implemented (claude_infer_tests `_for` unseen-id — confirm test id)

### L2.RPT.045 — Tolerates both `message.content[]` and bare top-level `content[]` block shapes
- layer: L2
- scenarios: [base]
- precondition: two blobs — one with `{"message":{"content":[…]}}`, one with `{"content":[…]}`
- action: `corroborate_jsonl` on each (same tool_use/result content)
- expected: identical verdict from both shapes
- assert: both blobs → the same `Corroboration` (e.g. `Stuck`)
- why: the transcript shape varies by Claude version; the `.get("message").and_then(content).or_else(content)` fallback must read both so corroboration is version-robust.
- status: implemented (claude_infer_tests both-shapes — confirm test id)

---

## Codex adapter (`CodexAdapter` / `CodexStateMachine`, authoritative PermissionRequest)

### L2.RPT.050 — codex UserPromptSubmit → Working (Inferred); native_id == thread.id (session_id alias)
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`; frame `codex {"hook_event_name":"UserPromptSubmit","session_id":"thr-1","cwd":"/repo"}`
- action: `Receiver::process(frame)` (dispatch routes to `codex` only — no infer fan-out)
- expected: `UpsertRun(Working)` with `agent_kind == Codex`, `native_id == "thr-1"`, `confidence == Inferred`; run-id shape `codex:thr-1:run-<n>`
- assert: `UpsertRun.agent_kind == AgentKind::Codex`; `.native_id == "thr-1"`; `.confidence == Inferred`; only the codex adapter fired (no claude/infer command)
- why: codex's `session_id` field is the durable `thread.id` (stable across `codex resume`, D4); the codex path is single-adapter (no S16 fan-out) — the dispatch must route claude vs codex correctly.
- status: implemented (codex_tests working transition — confirm test id)

### L2.RPT.051 — codex PermissionRequest (no decision) → Waiting + Approval + Confidence::High (authoritative)
- layer: L2
- scenarios: [base]
- precondition: Working `thr-1`; frame `codex {"hook_event_name":"PermissionRequest","session_id":"thr-1","tool_name":"Bash"}`
- action: `CodexAdapter::ingest_json`
- expected: `UpsertRun(run)` with `state==Waiting`, `urgency==Approval`, `confidence==High`, `last_message=="Approve Bash?"`, `waiting_since.is_some()`
- assert: `UpsertRun.state==State::Waiting`; `.urgency==Some(Urgency::Approval)`; `.confidence==Confidence::High`; `.last_message==Some("Approve Bash?")`
- why: PermissionRequest is the ONLY authoritative waiting signal → the ONLY High path in codex; this is the contrast with claude's inferred-only waiting and the load-bearing invariant-5 boundary.
- status: implemented (codex_tests permission-request high — confirm test id)

### L2.RPT.052 — codex PermissionRequest RESPONSE (decision allow|deny) → Working (auto-resolve, S13)
- layer: L2
- scenarios: [base]
- precondition: `thr-1` Waiting on approval; frame PermissionRequest carrying `"decision":"allow"` (also test nested `{"decision":{"permission":"deny"}}`)
- action: `CodexAdapter::ingest_json(response)`
- expected: `UpsertRun(Working)` with `resolved_approval==true`; both allow and deny resume to Working
- assert: `UpsertRun.state==State::Working`; both `"decision":"allow"` and `{"decision":{"permission":"deny"}}` → Working; `is_approval_response()==true`
- why: when the user answers in the real terminal codex emits the response then resumes; either decision drives the run out of waiting with no Fleet interaction; `RawDecision` must parse both plain-string and structured spellings.
- status: implemented (codex_tests approval-response auto-resolve — confirm test id)

### L2.RPT.053 — codex activity (PreToolUse) auto-resolves a stale pending approval
- layer: L2
- scenarios: [base]
- precondition: `thr-1` Waiting (pending_approval set); frame PreToolUse `thr-1`
- action: `CodexAdapter::ingest_json(pretool)`
- expected: `UpsertRun(Working)` with `resolved_approval==true` (fresh activity clears a stale waiting)
- assert: `UpsertRun.state==State::Working`; transition `resolved_approval==true`; `awaiting_approval()==false`
- why: codex may resume directly into activity without an explicit decision event; any subsequent activity hook must also clear a stale waiting (the second auto-resolve path).
- status: implemented (codex_tests activity-auto-resolve — confirm test id)

### L2.RPT.054 — codex Stop → Idle/Done, clears pending approval; SessionEnd → Dead/High
- layer: L2
- scenarios: [base]
- precondition: `thr-1` Working (or Waiting); frames Stop, then SessionEnd
- action: `CodexAdapter::ingest_json` each
- expected: Stop (no marker) → `UpsertRun(Idle)`, `urgency` cleared, `pending_approval=false`; Stop with `turn_complete` marker → Done; SessionEnd → `UpsertRun(Dead)` with `confidence==High`, `last_message=="Thread closed."`
- assert: bare Stop → Idle; marker Stop → Done; SessionEnd → `state==Dead && confidence==High && last_message==Some("Thread closed.")`
- why: codex mirrors claude's turn-complete/exit semantics (D9 Done≠Idle, confirmed-exit High); a Stop must also clear any stale waiting it interrupts.
- status: implemented (codex_tests stop/sessionend — confirm test id)

### L2.RPT.055 — EDGE: codex PostToolUse / PreCompact / PostCompact are liveness-only; Other yields nothing
- layer: L2
- scenarios: [base]
- precondition: Working `thr-1`; frames PostToolUse, PreCompact, PostCompact, and an unknown `"hook_event_name":"Frobnicate"`
- action: `CodexAdapter::ingest_json` each
- expected: PostToolUse/PreCompact/PostCompact → `Liveness{run_id}` only (no state flip); `Frobnicate` → `Other(_)`, `[]`
- assert: each compaction/post → exactly one `Liveness` no `UpsertRun`; the Other-hook → `[]`; `state_of("thr-1")==Some(Working)` throughout
- why: telemetry/compaction hooks must refresh liveness without moving state; unknown names are forward-compatible no-ops (schema-drift tolerance, mirrors S15).
- status: implemented (codex_tests telemetry/other — confirm test id)

### L2.RPT.056 — EDGE: codex parse errors (missing session_id / event name / bad JSON) drop cleanly
- layer: L2
- scenarios: [base]
- precondition: frames `codex {"hook_event_name":"Stop"}`, `codex {"session_id":"t"}`, `codex not-json`
- action: `CodexHookEvent::parse` / `CodexAdapter::ingest_json` each
- expected: `Err(MissingThreadId)` / `Err(MissingEventName)` / `Err(InvalidJson(_))`; `ingest_json` → `[]` for all
- assert: `parse(no_sid).unwrap_err()==CodexParseError::MissingThreadId`; `parse(no_evt).unwrap_err()==CodexParseError::MissingEventName`; `ingest_json("codex not-json"` via Receiver) → `[]`
- why: the codex adapter swallows parse errors identically to claude — a malformed codex frame must never crash the reporter or overstate a thread's state.
- status: implemented (codex_tests parse-error — confirm test id)

### L2.RPT.057 — codex thread-id alias robustness (`thread_id`/`threadId`/`sessionId`) all anchor identity
- layer: L2
- scenarios: [base]
- precondition: frames spelling the anchor as `"thread_id"`, `"threadId"`, `"sessionId"`, `"session_id"` (same value `thr-1`)
- action: `CodexHookEvent::parse` each
- expected: all parse with `thread_id == "thr-1"`
- assert: each parsed `.thread_id == "thr-1"`
- why: codex/cmux builds spell the durable anchor differently; the `#[serde(alias …)]` set must collapse them all to the one `native_id` so resume continuity holds.
- status: implemented (codex_tests alias anchor — confirm test id)

---

## `--serve` socket lifecycle (`serve_unix`, `bind_reclaiming`, `restrict_socket_perms`)

### L2.RPT.060 — serve_unix forwards a framed hook from the socket to the ReporterHandle
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` bound on a temp socket with a `Receiver` + a `Reporter` channel handle
- action: connect a UnixStream, write `claude {UserPromptSubmit session_id:sess-A}\n`, flush, drop (EOF)
- expected: within 2 s the handle's channel receives `UpsertRun(run)` with `state==Working`, `native_id=="sess-A"`
- assert: `rx.recv()` within `timeout(2s)` yields `ReporterCommand::UpsertRun(run)` with `run.state==State::Working && run.native_id=="sess-A"`
- why: this is the real socket→`Receiver::process`→handle path (not a unit test of the adapter); it proves a hook write actually drives a Hub-bound command end to end.
- status: implemented (serve test `serve_unix_forwards_a_framed_hook_to_the_handle`)

### L2.RPT.061 — serve_unix handles many short-lived connections sharing one Receiver's session state
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` bound; the env's hook relay opens a NEW `nc -U` connection per hook (one connect per payload)
- action: write UserPromptSubmit on connection #1 (close), then Stop on connection #2 (close), same `session_id`
- expected: the session advances Working→Idle across the two connections (the `Arc<Mutex<Receiver>>` is shared, so adapter state persists)
- assert: handle receives `UpsertRun(Working)` then `UpsertRun(Idle)` for the same `native_id`; the run_id is identical across both
- why: a window's hooks arrive on many short-lived socket connections (`printf … | nc -U` per hook) but belong to one session; the shared Receiver must carry state across connections or every hook would mint a fresh run.
- status: partial (single-connection forwarding tested; multi-connection state-persistence not yet asserted by a serve test)

### L2.RPT.062 — serve_unix reclaims a STALE socket file left by a dead reporter
- layer: L2
- scenarios: [base]
- precondition: a leftover socket *file* whose previous owner is gone (bind-and-drop a listener)
- action: `bind_reclaiming(&sock)`
- expected: `Ok(listener)` — the stale file is removed and rebound
- assert: `bind_reclaiming(&sock).is_ok()`
- why: the env entrypoint `rm -f $FLEET_REPORTER_SOCKET` then starts `--serve`; on an unclean prior exit a dead socket file must be reclaimed (mirrors the Hub lockfile D2 discipline), never fatal.
- status: implemented (serve test `serve_unix_reclaims_a_stale_socket_file`)

### L2.RPT.063 — EDGE: a LIVE second reporter on the same socket is refused
- layer: L2
- scenarios: [base]
- precondition: a `serve_unix` already bound and accepting on `sock`
- action: `bind_reclaiming(&sock)` from a second would-be owner
- expected: `Err(...)` with message "another fleet-reporter --serve already owns <path> (live)"
- assert: `bind_reclaiming` returns `Err`; the error string contains `already owns` and `(live)`
- why: two reporters on one socket would split a window's hooks and double-report; the live-probe (connect succeeds ⇒ live) must refuse the second, single-instance discipline.
- status: TODO (no serve test asserts the live-refusal branch; only the stale-reclaim branch is covered)

### L2.RPT.064 — The reporter socket is restricted to owner-only (mode 0600)
- layer: L2
- scenarios: [base]
- needs: [exec]
- precondition: a running env with `--serve` bound at `/tmp/fleet-reporter.sock`
- action: `env.exec("stat -c '%a' /tmp/fleet-reporter.sock")`
- expected: `600`
- assert: `exec` output trimmed == `"600"` (unix); `restrict_socket_perms` set `0o600`
- why: a hook frame can mutate this window's reported agent state, so the socket is a local trust boundary — no other local user may connect and inject spoofed frames (defence-in-depth).
- status: implemented (behaviour `reporter.socketMode0600`)

### L2.RPT.065 — EDGE: a blank line on the socket is skipped without counting as drift
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` bound; write `"\n"` then a real `claude {…Stop…}\n`
- action: write a blank line, then a valid framed line
- expected: the blank line is `continue`d (serve loop skips `line.trim().is_empty()`), the valid frame still produces its `UpsertRun`
- assert: only one command (`UpsertRun(Idle)`) arrives at the handle; no panic; the connection stays open for the second line
- why: `nc`/relay can emit stray newlines; the serve read loop must skip empty lines without closing the connection or polluting the drop counter.
- status: partial (serve loop skips blank lines in code; not asserted by a dedicated serve test)

### L2.RPT.066 — EDGE: an accept() error does not kill the receiver (loop continues)
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` running; induce a transient accept error
- action: trigger an `accept()` Err, then connect normally
- expected: the receiver logs `warn!("accept failed; continuing")` and keeps serving; the subsequent connection works
- assert: a connection after the induced error still delivers its `UpsertRun` to the handle
- why: an accept error on one connection must not bring down the whole reporter (availability); the loop `continue`s rather than returns.
- status: TODO (no test injects an accept error; the `continue`-on-error path is unguarded)

### L2.RPT.067 — The serve TICK task auto-fires inferred Waiting on the real timer (no hook needed)
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` running with a real `tokio::time::interval(250ms)` tick task; a PreToolUse-without-Stop written once
- action: write `claude {PreToolUse session_id:sess-W tool_use_id:toolu_x}\n`, then wait > DEFAULT_DEBOUNCE_MS (1.5 s) WITHOUT sending another frame
- expected: the tick task drives `Receiver::tick()` and the handle receives an `UpsertRun(Waiting, Inferred)` for `sess-W` purely from the timer
- assert: handle receives, after ~1.5–1.75 s, `UpsertRun` with `state==Waiting && confidence==Inferred && native_id=="sess-W"`, with no second socket write
- why: while a human sits on an approval no further hook arrives, so `serve_unix` must advance the infer clock itself via the spawned interval task; this is the ONLY thing that makes the inferred ping fire in production.
- status: TODO (the adapter tick is unit-tested with injected time; the real spawned-timer path through serve_unix is not yet asserted)

### L2.RPT.068 — EDGE: TICK task uses Skip missed-tick behaviour (no backlog pile-up under a held lock)
- layer: L2
- scenarios: [base]
- precondition: `serve_unix`'s ticker created with `set_missed_tick_behavior(MissedTickBehavior::Skip)`
- action: hold the `Receiver` lock past several tick intervals, then release
- expected: on release only ONE catch-up tick runs (latest clock advance), not a burst of backlogged ticks
- assert: at most one `tick()` worth of commands is emitted after a multi-interval stall (no N-fold duplicate Waiting)
- why: a stalled lock must not pile up backlogged ticks that then fire en masse; Skip keeps only the latest advance — guards against ping bursts.
- status: TODO (behaviour set in code; unverified by a test)

### L2.RPT.069 — EDGE: TICK task stops when the reporter handle is closed (no leak)
- layer: L2
- scenarios: [base]
- precondition: `serve_unix` running with the tick task; the reporter loop exits (its `rx` dropped)
- action: drop the reporter receiver so `handle.send` returns false; let a tick fire that would forward a command
- expected: the tick task `return`s (stops) once `handle.send` reports the loop gone
- assert: the spawned tick task terminates after `handle.send(cmd)` returns false (observable: no further sends; task joins)
- why: the tick task must not spin forever after the reporter is gone; `if !handle.send(cmd) { return }` is the shutdown latch for both the tick task and the connection task.
- status: TODO (latch exists in code; not covered by a test)

---

## Reporter → Hub commands (frame semantics through to the Hub session row)

### L2.RPT.070 — End-to-end: a controlled PreToolUse-without-Stop at the env socket reaches `[waiting]` on the Hub
- layer: L2
- scenarios: [base]
- needs: [exec]
- precondition: a booted env whose reporter `--serve` is bound and phoned home to the host Hub (session title == env.id); Hub reachable via `fleet ls --once`
- action: `env.exec("printf 'claude %s\n' '<PreToolUse json, session_id=wait-<id>, tool_use_id=toolu_fleetwait>' | timeout 2 nc -N -U /tmp/fleet-reporter.sock")`; poll the Hub
- expected: within ~45 s the env's Hub session row renders `[waiting]`; then a Stop frame resolves it cleanly (→ idle), leaving the session clean
- assert: `pollHub(sessionTitle, st => st === "waiting")` succeeds; the resolving Stop is sent in a `finally` so the env never hangs
- why: this is the ONLY end-to-end guard that serve_unix's tick-driven inference + urgency/rollup plumbing + the CLI's `[waiting]` rendering actually emit the approval-needed ping (the Rust tests exercise the infer machine in isolation). Determinism over realism: a controlled frame, not a flaky real Bash-approval block.
- status: implemented (behaviour `agent.waitingState`)

### L2.RPT.071 — End-to-end: real `claude -p` drives the Hub session working→idle/done/dead (≥1 run recorded)
- layer: L2
- scenarios: [base]
- needs: [termSend]
- precondition: a booted env with the claude shim installed (relays `claude `-tagged hooks to the socket); container claude authenticated; Hub reachable
- action: `env.request({type:"termSend", text:'claude -p "say hi"\n'})`; poll the Hub
- expected: the session goes active (working/waiting) then terminates (idle/done/dead); `(N runs)` recorded; `-p` is one-shot so SessionEnd fires
- assert: `pollHub` sees working/waiting then idle/done/dead; OR the row shows `(\d+ runs?)` corroborating a run; SKIP cleanly (not fail) when claude is unauthenticated or the Hub is unreachable
- why: guards the full real spine — container claude → shim hook relay (`printf 'claude %s\n' … | nc -U`) → reporter S15 adapter → WS phone-home → Hub registry → CLI render; a break here means Fleet went blind to agent activity.
- status: implemented (behaviour `agent.claudeRuns`)

### L2.RPT.072 — A state change emits exactly ONE UpsertRun; a no-op-with-liveness emits a Liveness, not an UpsertRun
- layer: L2
- scenarios: [base]
- precondition: fresh adapter; sequence UserPromptSubmit (Working), then a duplicate UserPromptSubmit (still Working)
- action: ingest both
- expected: first → one `UpsertRun(Working)`; second (no state change) → one `Liveness{run_id}`, zero `UpsertRun`
- assert: first result has exactly one `UpsertRun` and no `Liveness`; second has exactly one `Liveness` and no `UpsertRun`
- why: the reporter→Hub command surface must be delta-minimal — only genuine state changes are `UpsertRun`; repeats are liveness refreshes, so the Hub isn't churned and the rollup isn't perturbed.
- status: implemented (claude_tests change-vs-liveness — confirm test id)

### L2.RPT.073 — EDGE: a drifted frame produces ZERO Hub commands (drift guard, end-to-end through process)
- layer: L2
- scenarios: [base]
- precondition: fresh `Receiver`
- action: `process("   ")`, `process("claude not-json")`, `process("gemini {…}")`
- expected: every call returns `[]` — no `UpsertRun`, no `Liveness`, no fabricated session
- assert: each `process(...)` is empty; `frames_dropped()` == 1 (only the truly-empty frame is frame-drift); no panic across all three
- why: the whole point of the drift guard is that no malformed/unknown input can ever mint a Hub delta or crash the receiver; confidence honesty is structural (a drifted payload simply produces no Hub delta).
- status: implemented (serve test `process_garbage_json_is_dropped_not_panicked`)

### L2.RPT.074 — EDGE: concurrent frames for two sessions on one socket keep distinct runs on the Hub
- layer: L2
- scenarios: [base]
- needs: [exec]
- precondition: a booted env; two distinct `session_id`s driven via two interleaved socket writes
- action: send PreToolUse(sess-A) and PreToolUse(sess-B) interleaved at the env socket; poll Hub
- expected: the Hub session shows TWO distinct runs (run count ≥2 / two run-ids), each independently transitionable
- assert: Hub session row shows `(2 runs)` (or ≥2); resolving sess-A to idle leaves sess-B's state untouched
- why: one reporter multiplexes many claude invocations; concurrent sessions must produce independent runs under one session shell — cross-contamination would mislabel one agent with another's state.
- status: implemented (behaviour `reporter.twoSessionsTwoRuns`)

---

## Notes / traceability

- Entries marked `implemented (<crate test>)` map to existing Rust unit tests in
  `serve.rs` / `claude.rs` (`claude_tests.rs`) / `claude_infer.rs` (`claude_infer_tests.rs`)
  / `codex.rs` (`codex_tests.rs`). The exact in-file test ids for the `claude_tests` /
  `codex_tests` / `claude_infer_tests` includes are to be stamped during the
  implementation phase (the `serve.rs` ones are cited verbatim above).
- The two L2 in-env behaviours that already cover this area end-to-end are
  `agent.waitingState` (L2.RPT.070) and `agent.claudeRuns` (L2.RPT.071) in
  `behaviours/agentInput.mjs`.
- The socket-lifecycle edges (live-refusal, 0600 stat, accept-error, the real spawned
  TICK timer, Skip backlog, tick shutdown latch) and the end-to-end two-sessions case
  are the main `TODO`s: the code paths exist and are reasoned about, but no
  test/behaviour yet asserts the live socket / spawned-timer behaviour.

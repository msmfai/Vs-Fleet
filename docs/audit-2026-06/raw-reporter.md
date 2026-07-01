# fleet-reporter (agent 3) — crown-jewel contract drift (web-sourced: code.claude.com/docs/en/hooks, developers.openai.com/codex/hooks)
- [HIGH correctness] #1 `done` unreachable: Stop hooks carry NO completion markers (task_complete/reason/subtype phantom) -> every finished turn = Idle, never Done. claude.rs:225 codex.rs:287 — M
- [HIGH dead] #2 Codex SessionEnd never emitted (not in official event set) -> `dead` only via timeout. codex.rs:493 — S
- [HIGH dead] #3 entire transcript-corroboration subsystem dead in prod (serve.rs never calls corroborate); TWO dup Corroboration types+JSONL scanners. transcript.rs + claude_infer.rs — M
- [HIGH dup] #4 four parallel state machines ~80% copy-paste (Transition x4, to_run, adapter boilerplate, RawDecision table x2). claude/claude_shim/codex/claude_infer — L
- [MED/HIGH wrong] #5 PermissionRequest response-as-input path fictional (decision is hook OUTPUT not inbound event); codex RawDecision even lacks `behavior`. codex.rs:168 claude_shim.rs:139 — S-M
- [MED fragile] #6 fake.rs now_iso8601 hand-rolled sec-precision formatter is the prod timestamp source. fake.rs:162 — S
- [MED errhandling] #7 adapter parse failures swallowed (Err=>Vec::new) + never bump drift counter. serve.rs:229 — S
- [LOW] #8 corroborate_transcript pure alias, tautology test — S
- [LOW/MED transport] #9 no sun_path length guard on reporter/hub sockets. transport.rs:154 — S
- [MED testsmell] #10 ~40 tests over a fixed 6-step fixture; Debug-string equality; len==20 asserts
- [LOW] #11 turn_complete_done/stop_hook_active handled in 2 places — S
- [LOW] #12 codex.rs header cites fork config; official codex hooks now exist — S
VERDICT: names/identity fields CONFIRMED correct; Done(#1)/Codex-dead(#2)/corroboration(#3)/approval-response(#5) BROKEN or DEAD vs live schema. Highest ROI: 1,2,3,5 then 4.

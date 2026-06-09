# 21 — Hub state: merge, rollup precedence, ephemeral vs persist, reap/TTL, atomicity

L2 / Fleet-stack. The Hub (`crates/fleet-hub`) is the single authoritative broker:
reporters push `session.upsert` / `run.upsert` / `*.remove` deltas; faces `subscribe`
and get a snapshot + live delta stream. This area pins the Hub's **observable
behaviour through the real binary** — driven from the harness by feeding the Hub
deltas (over its WS listener, the same `ws://HOST:51777` the in-env reporter dials, or
over the `cfg(unix)` `hub.sock` fast path) and reading state back with the `fleet`
CLI (`target/debug/fleet ls --once`, which prints one `[<state>] <title> …` row per
session — the merged rollup) or via a raw `subscribe` round-trip (snapshot frame).

Conventions for this file:
- **Hub binary under test** = `fleet-hub` built from this tree, started by the
  integration harness (`containers/fleet-env/test.sh`) bound `FLEET_WS_ADDR=0.0.0.0`,
  `FLEET_WS_PORT=51777`. `FLEET_PERSIST` unset ⇒ ephemeral (in-memory log); set ⇒
  durable SQLite at `$FLEET_RUNTIME_DIR/fleet/hub.db`.
- **Reporter side** = either the real in-env claude run (phones home as
  `FLEET_SERVER_ID == env.id`) or a synthetic WS client the harness opens to inject
  exact `ClientMessage` frames (the deterministic path — preferred per overview
  "determinism over realism", and the only way to hit the seq/epoch/reap edges).
- **The observable** is always one of: a `fleet ls --once` row's bracketed state; the
  presence/absence of a session/run row; a `subscribe` snapshot field
  (`rollup_state`, `rollup_urgency`, `runs[]`, `muted`); or a broadcast `Event`
  `type_name` on a second subscriber socket.
- Most entries are **TODO** at the VM/E2E layer: the merge/rollup/persist/reap logic
  is exhaustively **Rust-unit-tested** today (cited per entry as `implemented(rust:
  <test>)` where a unit test pins the exact behaviour) but **not yet wired as a
  container/E2E spec test**. Only `agent.claudeRuns` / `agent.waitingState` drive the
  Hub end-to-end through a real env today.

Hub state ranks (from `fleet_protocol::rollup::state_rank`, the rollup contract):
`Waiting(5) > Error(4) > Working(3) > Done(2) > Idle(1) > Dead(0)`.
Urgency ranks: `Approval(3) > Question(2) > IdleDone(1) > None(0)`.

---

## Session / run upsert + merge

### L2.HUB.001 — session.upsert on a fresh Hub adds the session and emits session.added
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: Hub up, ephemeral, snapshot empty (`fleet ls --once` prints 0 rows)
- action: harness WS client sends `{"type":"session.upsert","session":{session_id:"s1",…,state:"idle",updated_at:"2026-06-08T00:00:00Z"}}`
- expected: snapshot has exactly one session `s1`; its `rollup_state == idle`
- assert: a second `subscribe` socket receives a `session.added` Event
  (`Event::type_name()=="session.added"`) **before** any other frame; `fleet ls --once`
  prints one row `[idle] s1`
- why: the first delta of a session must register it and announce it once (added, not
  updated) — the entry-point of every Hub flow; guards `MergeEngine::upsert_session`'s
  added/updated branch + `order.push` on the E2E path.
- status: implemented(rust: merge::tests::session_add_then_update,
  server::tests::deltas_are_broadcast_to_subscribers); TODO(E2E via fleet CLI)

### L2.HUB.002 — repeat session.upsert of same id replaces in place, emits session.updated (not a second add)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` already registered (L2.HUB.001 state)
- action: send a second `session.upsert` for `s1` with `state:"working"` and a new title
- expected: still exactly one session row (no duplicate); title/state updated
- assert: second subscriber gets `session.updated` (NOT `session.added`); `fleet ls
  --once` still prints exactly one `s1` row; snapshot length == 1; insertion order
  unchanged (id stays in same `order` slot)
- why: re-registration on reporter reconnect must update, never spawn a ghost twin;
  pins the `contains_key` branch + order-preservation in `upsert_session` /
  `snapshot_is_insertion_ordered`.
- status: implemented(rust: merge::tests::session_add_then_update,
  merge::tests::snapshot_is_insertion_ordered); TODO(E2E)

### L2.HUB.003 — run.upsert on a known session adds the run and recomputes session rollup
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: session `s1` registered with `state:"idle"`, no runs
- action: send `{"type":"run.upsert","session_id":"s1","run":{run_id:"r1",state:"working",…}}`
- expected: `s1.rollup_state` flips `idle → working`
- assert: `fleet ls --once` row for `s1` becomes `[working]`; subscriber receives TWO
  Events in order: `run.added` then `session.updated`; snapshot `s1.runs.len()==1`
- why: a run delta both records the run AND re-rolls the session in one apply, so faces
  tracking session-level state stay correct; pins `MergeEngine::upsert_run`'s
  `(run_event, session.updated)` pair + `recompute_rollups`.
- status: implemented (hub.runUpsertRecomputesRollup); also implemented(rust: merge::tests::run_upsert_recomputes_rollup)

### L2.HUB.004 — run.upsert with an existing run_id replaces in place (no append) and re-rolls
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` has one run `r1` at `state:"waiting", urgency:"approval"` →
  `rollup_state==waiting`, `rollup_urgency==approval`
- action: send `run.upsert` for the SAME `run_id:"r1"` with `state:"working", urgency:null`
- expected: `s1.runs.len()` stays 1 (in-place, not appended); rollup falls to
  `working` with `rollup_urgency` absent
- assert: subscriber gets `run.updated` (NOT `run.added`); snapshot `s1.runs.len()==1`,
  `rollup_state==working`, `rollup_urgency` field absent/null; `fleet ls` row `[working]`
- why: a run's state change must mutate the existing slot, not stack a duplicate run;
  pins `run_update_in_place_changes_rollup`.
- status: implemented(rust: merge::tests::run_update_in_place_changes_rollup); TODO(E2E)

### L2.HUB.005 — EDGE: run.upsert targeting an UNKNOWN session is a no-op (no ghost session, no broadcast)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: Hub empty (no session `ghost` ever registered)
- action: send `{"type":"run.upsert","session_id":"ghost","run":{run_id:"r1",…}}`
- expected: nothing happens — no session `ghost` materializes from a run delta
- assert: `fleet ls --once` prints 0 rows; second subscriber receives NO Event
  (`rx.try_recv()` empty); with `FLEET_PERSIST` set, `hub.db` event count unchanged
  (no row appended — the no-op never pollutes the log)
- why: the Hub must reject orphan run deltas (reporter must register the session
  first); a run can never conjure a session. Pins `run_delta_on_unknown_session_is_noop`
  + persist `no_op_removes_do_not_grow_the_log`.
- status: implemented(rust: merge::tests::run_delta_on_unknown_session_is_noop,
  persist::tests::no_op_removes_do_not_grow_the_log); TODO(E2E)

### L2.HUB.006 — session.upsert carrying inline runs rolls up immediately on insert
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: Hub empty
- action: send one `session.upsert` whose `runs:[{r1,waiting,approval},{r2,working,null}]`
  in the single object
- expected: the session lands already rolled-up (no separate run delta needed)
- assert: snapshot `s1.rollup_state==waiting`, `s1.rollup_urgency==approval`;
  `fleet ls` row `[waiting]` on the very first frame
- why: a reporter that ships a session+runs atomically must not show a stale/blank
  rollup for one tick; pins `upsert_session_with_runs_rolls_up_immediately` +
  `recompute_rollups` being called inside `upsert_session`.
- status: implemented(rust: merge::tests::upsert_session_with_runs_rolls_up_immediately);
  TODO(E2E)

### L2.HUB.007 — run.remove drops the run and re-rolls; removing the last run leaves an empty session (not removed)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` has `r1(working)` + `r2(waiting,question)` → rollup `waiting`
- action: send `{"type":"run.remove","session_id":"s1","run_id":"r2"}`
- expected: rollup falls back to `working`; then remove `r1` too → session `s1` stays
  present with `runs:[]` (only an explicit `session.remove` deletes the session)
- assert: after removing r2: subscriber gets `run.removed`+`session.updated`, row
  `[working]`; after removing r1: `s1` still in `fleet ls` (rollup_state retains last
  value per `recompute_rollups` empty-set rule), `s1.runs.len()==0`
- why: run removal re-rolls; an empty session is NOT auto-deleted — session lifecycle
  is owned by session.remove only. Pins `run_remove_recomputes` + merge doc
  "removing the last run leaves an empty session".
- status: implemented(rust: merge::tests::run_remove_recomputes,
  merge::tests::run_delta_on_unknown_session_is_noop); TODO(E2E)

### L2.HUB.008 — EDGE: remove of an absent run / absent session is an idempotent no-op (no broadcast, no log row)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, persist]
- precondition: `s1` registered, FLEET_PERSIST set; record `hub.db` event count N
- action: send `run.remove` for `(s1, ghostRun)`, then `(ghostSession, r)`, then
  `session.remove` for `ghostSession`
- expected: all three are no-ops
- assert: subscriber receives no Event for any of them; `hub.db` event count still ==
  N (`apply_*_remove` only logs a removal that changes state); `fleet ls` unchanged
- why: idempotent removes must not grow the durable log nor emit phantom deltas;
  pins `no_op_removes_do_not_grow_the_log` + `remove_session`/`remove_run` `None`/empty
  branches.
- status: implemented(rust: persist::tests::no_op_removes_do_not_grow_the_log,
  merge::tests::session_remove_is_idempotent, merge::tests::remove_nonexistent_run_is_noop);
  TODO(E2E)

### L2.HUB.009 — EDGE: undecodable / unknown-type client frame is dropped without crashing the connection
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: Hub up, one socket open
- action: send a text frame `{"type":"totally.unknown","x":1}`, then a valid
  `session.upsert` on the same socket
- expected: the bad frame is logged + ignored; the connection survives; the following
  valid delta applies
- assert: the unknown frame produces no Event and no reply; the subsequent
  `session.upsert` still yields a `session.added` and a `fleet ls` row — i.e. the
  socket was not torn down (`serve_ws_connection` `from_str` warn-and-continue path)
- why: a malformed/forward-incompatible frame must never crash a reporter's
  connection; pins wire `unknown_type_is_rejected` (deserialize error) + server's
  warn-and-continue loop.
- status: implemented(rust: wire::tests::unknown_type_is_rejected); partial(server
  warn-and-continue path has no direct unit test; E2E TODO)

---

## Rollup-state precedence

### L2.HUB.010 — Waiting wins the rollup over every other state (the ping-precedence invariant)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` with runs `r1(working)`, `r2(idle)`, `r3(done)`, `r4(error)`
  → rollup `error` (rank 4)
- action: add `r5(waiting)` via run.upsert
- expected: `s1.rollup_state` escalates to `waiting` regardless of the four other
  concurrent states
- assert: `fleet ls` row flips to `[waiting]`; snapshot `rollup_state==waiting`; this is
  the ONLY state for which `State::pings()` is true (so this is the row a face badges)
- why: waiting is the single attention-demanding state and MUST dominate the rollup so
  a blocked agent is never masked by a louder-counted but lower-rank state; pins
  `state_rank(Waiting)==5` (top) + `waiting_beats_working`.
- status: implemented (hub.waitingWinsRollup); also implemented(rust: rollup::tests::waiting_beats_working,
  state::tests::only_waiting_pings)

### L2.HUB.011 — Error > Working > Done > Idle precedence holds pairwise
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` empty of runs
- action: add runs to force each adjacent pair: {error,working}→error; then drop error
  so {working,done}→working; then {done,idle}→done
- expected: rollup at each step == the higher-ranked of the present states
- assert: `fleet ls` row reads `[error]`, then `[working]`, then `[done]` across the
  three steps; snapshot `rollup_state` matches each
- why: the middle of the precedence ladder must be exact (Error masks Working masks
  Done masks Idle) so the rail badge reflects the worst live run; pins `state_rank`
  ordering 4>3>2>1.
- status: implemented(rust: rollup::tests::done_distinct_and_ranks_above_idle, plus
  state_rank ordering); TODO(E2E)

### L2.HUB.012 — Done is kept DISTINCT from Idle on the wire and in rollup (D9)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` with a single run
- action: upsert the run as `state:"done"`, read; then as `state:"idle"`, read
- expected: the two produce different rollup rows; `done` ranks above `idle`
- assert: `fleet ls` shows `[done]` then `[idle]` (distinct tokens, never collapsed);
  snapshot run `state` serializes to the kebab token `"done"` vs `"idle"`; a
  `{idle,done}` two-run session rolls to `done`
- why: D9 locks Done≠Idle — "task complete" must be visually distinguishable from
  "awaiting next prompt"; pins `state::tests::state_wire_tokens` +
  `done_distinct_and_ranks_above_idle`.
- status: implemented (hub.doneDistinctFromIdle); also implemented(rust: state::tests::state_wire_tokens,
  rollup::tests::done_distinct_and_ranks_above_idle)

### L2.HUB.013 — Dead is the LOWEST rank: a dead run never masks any live run's rollup
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` with `r1(idle)`
- action: add `r2(dead)` → rollup must stay `idle`; then remove `r1` so only the dead
  run remains → rollup becomes `dead`
- expected: dead (rank 0) loses to idle (rank 1); only an all-dead session rolls dead
- assert: after adding r2: `fleet ls` row still `[idle]`; after removing r1: row `[dead]`
- why: a crashed/exited run must not drag a healthy session's badge down to dead;
  pins `state_rank(Dead)==0` (bottom).
- status: implemented(rust: rollup state_rank ordering — Dead=0); TODO(E2E)

### L2.HUB.014 — rollup_urgency is the most-urgent across runs (Approval > Question > IdleDone > None)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` with `r1(waiting,question)`
- action: add `r2(waiting,approval)`
- expected: `s1.rollup_urgency` escalates `question → approval`
- assert: snapshot `rollup_urgency=="approval"`; rollup_state stays `waiting`
- why: when several runs ping, the face must surface the loudest reason (an approval
  outranks a mere question); pins `approval_is_most_urgent`.
- status: implemented(rust: rollup::tests::approval_is_most_urgent); TODO(E2E)

### L2.HUB.015 — EDGE: an empty (run-less) session keeps its last rollup_state and clears rollup_urgency
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` registered `state:"working"` with one run `r1(working,approval)`
  → rollup `working`/`approval`
- action: `run.remove` r1 (last run)
- expected: `rollup_state` is left at its prior value (`working` — `recompute_rollups`
  does NOT invent a state for an empty run set) but `rollup_urgency` clears to absent
- assert: snapshot `s1.rollup_state==working` (unchanged), `rollup_urgency` field
  absent/null; `fleet ls` row still `[working]`
- why: an empty session must not be assigned a fabricated state, yet its urgency (a
  per-run signal) must drop — the asymmetric empty-set rule in `recompute_rollups`;
  pins the merge doc + `rollup_urgency` `None`-normalization.
- status: implemented(rust: merge::recompute_rollups behaviour, exercised via
  run_remove_recomputes); partial(no test asserts the empty-set state-retention edge
  directly); TODO(E2E)

### L2.HUB.016 — INVARIANT: after any delta sequence, every session's stored rollup equals the recomputed max (G0)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: Hub empty
- action: drive a randomized/long sequence of session+run upserts/removes across 2–3
  sessions (the harness replays a fixed pseudo-random script for determinism)
- expected: at every observation point `rollup_state == max_by(state_rank, runs)` and
  `rollup_urgency == max_by(urgency_rank, runs)` (None-normalized)
- assert: a `subscribe` snapshot run through the same `rollup_holds` predicate the Rust
  G0 property test uses returns true for every session; `fleet ls` never shows a row
  whose bracketed state disagrees with the worst run the snapshot lists
- why: this is THE rollup invariant (G0 property-test target) — the one property the
  whole "all faces see the same thing" guarantee rests on; pins
  `all_rollups_hold` / `rollup_holds`.
- status: implemented(rust: merge::MergeEngine::all_rollups_hold + property coverage,
  persist append_then_replay_equals_live_state asserts all_rollups_hold post-replay);
  TODO(E2E property harness)

---

## Ephemeral default vs FLEET_PERSIST replay

### L2.HUB.017 — Ephemeral default: a Hub restart starts EMPTY (no resurrection of prior sessions)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart]
- precondition: Hub started with FLEET_PERSIST UNSET; inject `s1`+run via WS so
  `fleet ls` shows one row
- action: kill the Hub process; restart it identically (still no FLEET_PERSIST)
- expected: the restarted Hub is a clean slate — `s1` is gone until a live reporter
  re-registers it
- assert: immediately post-restart `fleet ls --once` prints 0 rows; a fresh `subscribe`
  returns an empty snapshot
- why: the inbox is a LIVE MIRROR by default — a restart must NOT resurrect ghost
  sessions or stale reclaim marks; live reporters repopulate. Pins `lib::run`'s
  `FLEET_PERSIST`-unset branch (`HubState::new`, in-memory log) + the
  `subscribe_returns_empty_snapshot` baseline.
- status: implemented(rust: server::tests::subscribe_returns_empty_snapshot for the
  empty baseline; lib::run ephemeral branch); TODO(E2E restart of the real binary)

### L2.HUB.018 — FLEET_PERSIST: restart replays the log and restores every session/run exactly
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart, persist]
- precondition: Hub started with FLEET_PERSIST set, `hub.db` on a tmp path; inject two
  sessions (`s1` working, `s2` done) and several runs via WS
- action: kill the Hub; restart with FLEET_PERSIST still set, same db_path
- expected: every session/run is restored byte-identically before the first connection
  is served (`from_log` replays in `seq` order into a fresh engine)
- assert: post-restart `fleet ls --once` prints the same two rows with the same
  bracketed states; a `subscribe` snapshot deep-equals the pre-kill snapshot;
  rollup invariant still holds post-replay
- why: durable mode must survive a Hub bounce with no state loss — the round-trip
  invariant; pins `restart_restores_all_sessions_and_runs` +
  `append_then_replay_equals_live_state`.
- status: implemented(rust: persist::tests::restart_restores_all_sessions_and_runs,
  append_then_replay_equals_live_state, restart_restore_is_idempotent_across_three_opens);
  TODO(E2E restart of the real binary)

### L2.HUB.019 — FLEET_PERSIST: a removed run stays removed after restart (no resurrection from the log)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart, persist]
- precondition: persistent Hub; `s2` has run `r2`; then `run.remove (s2,r2)` is applied
  (logged as a `run.remove` row)
- action: restart the persistent Hub
- expected: replay applies upsert THEN remove in seq order → `r2` does not come back
- assert: post-restart `fleet ls` shows `s2` with no run row; snapshot `s2.runs` empty
- why: the log is the single source of truth (mutations, not a state dump) — a later
  remove must win on replay so a reaped/removed entry never resurrects; pins
  `restart_restores_all_sessions_and_runs` (its removed-run assertion).
- status: implemented(rust: persist::tests::restart_restores_all_sessions_and_runs);
  TODO(E2E)

### L2.HUB.020 — EDGE: FLEET_PERSIST crash-mid-write — a torn/garbage tail row is skipped, the intact prefix restores
- layer: L2
- scenarios: [base, crash-boot]
- isolation: fresh
- needs: [hubReachable, hubRestart, persist]
- precondition: persistent Hub with two intact rows logged; harness appends a truncated
  JSON payload directly into the `events` table (simulating a torn final write)
- action: restart the Hub (reopen the db)
- expected: the two intact rows restore; the unparseable tail row is skipped with a
  warning, not fatal
- assert: post-restart `fleet ls` shows the prefix state (1 session, 1 run); a debug
  `replay_into` reports `(applied=2, skipped=1)`; the Hub serves normally (no boot
  abort)
- why: a crash mid-append must never brick the Hub — every intact prefix recovers;
  pins `crash_mid_write_partial_tail_tolerated`.
- status: implemented(rust: persist::tests::crash_mid_write_partial_tail_tolerated);
  TODO(E2E)

### L2.HUB.021 — EDGE: FLEET_PERSIST forward-compat — an unknown future `kind` row is skipped, not fatal
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart, persist]
- precondition: persistent Hub with one valid `session.upsert` row; harness injects a
  row with `{"kind":"session.future_op",…}`
- action: restart the Hub
- expected: the unknown-kind row is skipped; the valid prefix is kept
- assert: post-restart snapshot length == 1; the Hub boots normally
- why: a newer build's log opened by an older binary must degrade gracefully (skip the
  row it can't interpret) rather than refuse to start; pins
  `unknown_future_kind_is_skipped_not_fatal`.
- status: implemented(rust: persist::tests::unknown_future_kind_is_skipped_not_fatal);
  TODO(E2E)

### L2.HUB.022 — EDGE: reopening the persistent log twice is deterministic (idempotent restore)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart, persist]
- precondition: persistent Hub, some state logged
- action: open the db → snapshot A (close); open again → snapshot B
- expected: A == B
- assert: the two `subscribe` snapshots deep-equal; no double-application of any delta
- why: replay must be a pure function of the log — reopening N times never drifts;
  pins `restart_restore_is_idempotent_across_three_opens`.
- status: implemented(rust: persist::tests::restart_restore_is_idempotent_across_three_opens);
  TODO(E2E)

---

## Dead-reap grace + session TTL / GC

### L2.HUB.023 — A dead run is reaped only AFTER the grace elapses (D17, default 1 h)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `s1` with three runs: `dead-old(updated 10:00)`, `dead-new(updated
  11:30)`, `alive(working, 09:00)`
- action: trigger a GC pass at `now=11:45` with grace=1 h (cutoff 10:45) — the harness
  drives `HubState::gc(now, grace, ttl)` with an injected `now`
- expected: only `dead-old` (10:00 < 10:45) is reaped; `dead-new` (11:30) and the live
  run survive
- assert: subscriber receives a `run.removed` + `session.updated`; `fleet ls`/snapshot
  shows `s1.runs == [dead-new, alive]` (order preserved); `dead-old` gone
- why: a freshly-dead run must linger briefly (so a quick relaunch reclaims it) but a
  long-dead run is GC'd — the grace window; pins `reap_dead_only_after_grace`.
- status: implemented(rust: persist::tests::reap_dead_only_after_grace); TODO(E2E GC
  trigger)

### L2.HUB.024 — EDGE: reap boundary is strict — updated_at == cutoff is NOT reaped; one second past IS
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `s1` with one `dead` run at `updated_at 10:00`, grace 1 h
- action: GC at `now=11:00` (cutoff exactly 10:00); then GC at `now=11:00:01`
- expected: at 11:00 the run survives (`updated == cutoff` is not strictly older); at
  11:00:01 it is reaped
- assert: first GC emits zero events, `s1.runs.len()==1`; second GC emits a
  `run.removed`, `s1.runs` empty
- why: the off-by-one at the grace boundary must be exact (`timestamp_lt` is strict
  `<`) so reaping is predictable and never one tick early; pins
  `reap_just_before_and_after_boundary`.
- status: implemented(rust: persist::tests::reap_just_before_and_after_boundary);
  TODO(E2E)

### L2.HUB.025 — EDGE: a dead run with an UNPARSEABLE updated_at is never reaped (conservative)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `s1` with a `dead` run whose `updated_at:"whenever"` (not ISO-8601)
- action: GC at any far-future `now` with default grace
- expected: the run is left alone (we never reap on a guess)
- assert: GC emits zero events; `s1.runs.len()==1` still
- why: a malformed timestamp must not cause an over-eager reap — `timestamp_lt`
  returns false on any parse failure; pins `malformed_updated_at_is_never_reaped`.
- status: implemented(rust: persist::tests::malformed_updated_at_is_never_reaped);
  TODO(E2E)

### L2.HUB.026 — Only the Dead state is reaped — a Waiting/Working/Idle/Done/Error run is never GC'd by reap
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `s1` with one run in each non-dead state, all `updated_at` far in the
  past (well beyond grace)
- action: GC at far-future `now`
- expected: reap_dead touches none of them (the `run.state == State::Dead` filter)
- assert: GC's reap phase emits zero `run.removed`; all five runs remain
- why: reaping is for *dead* runs only — a long-idle but alive run must not be culled
  by dead-reaping (session TTL is the separate mechanism for quiet sessions); pins the
  `state == State::Dead` guard in `reap_dead`.
- status: implemented(rust: reap_dead's Dead-filter, covered indirectly by
  reap_dead_only_after_grace's `alive` run); partial(no test asserts ALL five non-dead
  states survive); TODO(E2E)

### L2.HUB.027 — Session TTL sweep drops a session untouched past the TTL, atomically with its runs
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `stale` session (`updated_at 00:00`, with a run) and `fresh`
  (`updated_at 11:00`)
- action: GC at `now=12:00`, session_ttl=1 h (cutoff 11:00) — sweep phase
- expected: `stale` (00:00 < 11:00) is swept entirely (its run goes with it via
  `session.remove`); `fresh` (11:00 == cutoff, not strictly older) survives
- assert: subscriber gets exactly one `session.removed` (for `stale`); snapshot has
  only `fresh`; `engine.session("stale")` is None — nothing of stale (entry or run)
  remains
- why: a long-quiet session must eventually be GC'd, and its whole entry (+runs) goes
  together in one removal — no half-dropped session; pins
  `sweep_expires_stale_sessions_atomically`.
- status: implemented(rust: persist::tests::sweep_expires_stale_sessions_atomically);
  TODO(E2E)

### L2.HUB.028 — Session TTL is far more lenient than dead-reap (24 h default) — a quiet but live session is not swept early
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: `s1` last `updated_at` 2 h ago, still live (working run)
- action: GC at `now` with default grace (1 h) and default session_ttl (24 h)
- expected: NOT swept (2 h < 24 h TTL) and its working run is NOT reaped (not dead)
- assert: GC emits zero events; `s1` and its run remain
- why: the two GC timers are deliberately asymmetric — a busy session that hasn't
  re-pinged for an hour must not vanish; pins `HubConfig` defaults (reap 1 h vs ttl
  24 h) + the sweep using a separate, longer cutoff.
- status: implemented(rust: lib::HubConfig::default session_ttl == 24 h; sweep timing
  in sweep_expires_stale_sessions_atomically); partial(no test combines both timers in
  one pass); TODO(E2E)

### L2.HUB.029 — GC survives restart: a reaped run / swept session does NOT resurrect after a persistent restart
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubRestart, hubGc, persist]
- precondition: persistent Hub; a dead run reaped (logged `run.remove`) and a stale
  session swept (logged `session.remove`)
- action: restart the persistent Hub
- expected: GC flows through the SAME append path, so replay re-applies the upsert then
  the GC removal in order — neither resurrects
- assert: post-restart the reaped run is absent and the swept session is gone in
  `fleet ls`/snapshot
- why: GC must be durable — a restart can't undo a reap (the log carries the removal);
  pins `reaped_run_stays_reaped_after_restart` + `sweep_survives_restart`.
- status: implemented(rust: persist::tests::reaped_run_stays_reaped_after_restart,
  sweep_survives_restart); TODO(E2E)

### L2.HUB.030 — EDGE: GC timer does not run on the instant the daemon starts (skips the immediate first tick)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, hubGc]
- precondition: persistent Hub whose log restores a session with a long-dead run that
  is already past grace
- action: start the Hub and observe within the first GC interval (<60 s) before the
  first real tick
- expected: the dead run is NOT reaped at t≈0 — the GC loop calls `tick().await` once
  to consume the immediate tick before entering the loop
- assert: in the first ~1 s after boot, `fleet ls` still shows the past-grace dead run;
  it is reaped only on the first scheduled pass (~60 s later)
- why: a restored-then-immediately-reaped run would race the reporter's reconnect-and-
  reclaim; the start-skip gives live reporters a window; pins the `tick.tick().await`
  before the loop in `lib::run`'s GC task.
- status: TODO (no unit test isolates the timer's first-tick skip; E2E timing test)

---

## Subscribe → snapshot atomicity & no-ghost

### L2.HUB.031 — subscribe returns the current snapshot THEN attaches to the delta stream (no lost/double-applied delta)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1`+`r1(working)` already applied
- action: open a NEW socket and send `subscribe`; immediately after, a reporter applies
  `r1 → waiting` from a different socket
- expected: the new subscriber gets the snapshot (showing `working`) first, then the
  `waiting` delta — never a gap where the `waiting` update is dropped, never the
  `waiting` update applied twice
- assert: frame[0] is `Event::Snapshot` with `s1.rollup_state==working`; the NEXT
  frame is a `session.updated`/`run.updated` carrying `waiting`; final state `waiting`
  applied exactly once
- why: snapshot is taken under the SAME async Mutex that serializes deltas, so the
  subscribe→snapshot boundary is atomic w.r.t. the broadcast stream — the core
  consistency guarantee; pins `apply(Subscribe)` taking the store lock + the server
  doc "snapshot then attach".
- status: implemented(rust: server::tests::delta_then_subscribe_reflects_state,
  deltas_are_broadcast_to_subscribers); partial(no test races a delta across the
  subscribe boundary directly); TODO(E2E)

### L2.HUB.032 — A late subscriber sees accumulated state (subscribe after deltas reflects them)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: a reporter applies `session.upsert s1` then `run.upsert r1(working)`
  BEFORE any face subscribes
- action: a face subscribes
- expected: its first frame already shows `s1` with `rollup_state==working`
- assert: snapshot `sessions.len()==1`, `sessions[0].session_id=="s1"`,
  `rollup_state==working`
- why: the Hub is the durable-in-memory authority — a face that connects late must not
  miss state that landed before it; pins `delta_then_subscribe_reflects_state`.
- status: implemented(rust: server::tests::delta_then_subscribe_reflects_state); TODO(E2E)

### L2.HUB.033 — EDGE: a slow subscriber that lags past the broadcast backlog gets a Lagged signal, not a corrupt stream
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: one subscriber that stops reading; Hub applies > BROADCAST_CAPACITY
  (1024) deltas
- action: the slow subscriber resumes
- expected: it observes a `Lagged(n)` (deltas dropped) and can re-subscribe for a fresh
  snapshot rather than silently desyncing
- assert: the server logs `subscriber lagged`; the connection is not torn (the
  `RecvError::Lagged` arm continues the loop); a re-`subscribe` returns a consistent
  snapshot
- why: a stalled face must degrade to "re-snapshot", never to a half-applied stream;
  pins the `broadcast::error::RecvError::Lagged` arm in `serve_ws_connection`.
- status: TODO (no test forces backlog overflow; E2E lag injection)

### L2.HUB.034 — NO GHOST RESURRECTION: a reporter reconnect under the same durable id reclaims its run, never duplicates it
- layer: L2
- scenarios: [base, preexisting-agent]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `s1` with run `r1` stamped `{durable_id:"d1", epoch:0, seq:3}` applied
- action: the reporter "reconnects" and re-sends seq 1..3 (it doesn't know which
  landed) then seq 4, all `epoch:0`, same `durable_id:"d1"`
- expected: seq 1..3 are duplicates (dropped, no broadcast); seq 4 applies; the session
  still has EXACTLY ONE run `r1` — no ghost twin
- assert: snapshot `s1.runs.len()==1`; only the seq-4 delta produces a broadcast;
  `reclaim.high_seq(d1)==4`, `reclaim.len()==1` (one durable id, no duplicate)
- why: an at-least-once reporter channel must yield exactly-once *effect* — replaying
  an already-applied prefix on reconnect must not spawn a second run; pins
  `redelivering_a_whole_prefix_is_a_noop`, `no_ghost_under_reconnect_storm`.
- status: implemented(rust: reclaim::tests::redelivering_a_whole_prefix_is_a_noop,
  no_ghost_under_reconnect_storm, reconnect_same_epoch_continues_series_and_reclaims);
  TODO(E2E via stamped WS deltas)

### L2.HUB.035 — EDGE: an out-of-order / stale-seq delta is dropped (last-writer-by-seq, no state regression)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: run `r1`/durable `d1` applied at `seq:7, state:working`
- action: a delayed delta `{durable_id:"d1", epoch:0, seq:3, state:idle}` arrives late
- expected: the stale lower-seq delta is rejected — the run stays `working` (it does
  not regress to the older `idle`)
- assert: snapshot `r1.state==working` unchanged; no broadcast for the stale delta;
  `reclaim.high_seq(d1)==7`
- why: network reorder must never let an old state overwrite a newer one — convergence
  is to the highest seq regardless of arrival order; pins
  `out_of_order_arrival_resolves_by_seq_not_arrival`, `lower_seq_after_higher_is_dropped`.
- status: implemented(rust: reclaim::tests::out_of_order_arrival_resolves_by_seq_not_arrival,
  lower_seq_after_higher_is_dropped); TODO(E2E)

### L2.HUB.036 — EDGE: a genuine fresh-start (bumped epoch) WIPES the prior seq series; a straggler from the old epoch is dropped
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: `d1` at `epoch:0, seq:9`
- action: send `{epoch:1, seq:1, state:working}` (clean-start relaunch), then a late
  `{epoch:0, seq:10}` straggler from the dead instance
- expected: the epoch-1 seq-1 delta APPLIES even though seq 1 < old HWM 9 (the wipe);
  the epoch-0 seq-10 straggler is dropped as a stale epoch
- assert: after the fresh-start: snapshot reflects the new run state, `reclaim.epoch(d1)
  ==1`, `high_seq==1`; the straggler produces no broadcast, `epoch` stays 1
- why: a relaunched agent reusing a durable id must start a clean series (not be
  rejected as a duplicate), and a stale straggler from the old run instance must never
  win; pins `fresh_start_bumps_epoch_and_wipes_series`,
  `stale_epoch_delta_after_fresh_start_is_dropped`.
- status: implemented(rust: reclaim::tests::fresh_start_bumps_epoch_and_wipes_series,
  stale_epoch_delta_after_fresh_start_is_dropped); TODO(E2E)

### L2.HUB.037 — INVARIANT 3: sweeping a session drops its reclaim marks atomically; a later fresh delta is admitted from scratch
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs, hubGc]
- precondition: session `s1` with stamped runs (durable ids `d1`,`d2`); an unrelated
  `other` durable id under a different session
- action: sweep `s1` via session TTL; then a brand-new stamped delta for `d1` at low
  seq arrives
- expected: `s1`'s reclaim marks (`d1`,`d2`) are dropped in lock-step with the session
  entry; `other` is untouched; the later `d1` delta is admitted FRESH (not rejected as
  a stale duplicate)
- assert: post-sweep `reclaim.contains(d1)==false && contains(d2)==false &&
  contains(other)==true`; the post-sweep `d1` delta yields `Decision::ApplyFresh`
- why: there must be no window where a session is gone but its seq state lingers (which
  would wrongly reject a legitimate later delta) — entry + dedup-queue vanish together;
  pins `drop_ids_drops_a_whole_session_atomically`, `drop_id_lets_later_fresh_delta_through`.
- status: implemented(rust: reclaim::tests::drop_ids_drops_a_whole_session_atomically,
  drop_id_lets_later_fresh_delta_through, drop_ids_is_idempotent_for_absent_ids);
  TODO(E2E)

### L2.HUB.038 — EDGE: a stamped run.upsert for an UNKNOWN session does NOT advance the reclaim seq (the run never landed)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [hubReachable, rawWs]
- precondition: no session `ghost` registered
- action: send a stamped `run.upsert` `(session_id:"ghost", durable_id:"d1", seq:5)`
- expected: no-op AND the reclaim HWM for `d1` is NOT bumped — so when `ghost` is later
  registered and the same `d1` seq series is replayed, seq 5 still applies
- assert: post-attempt `reclaim.high_seq(d1)` is None/untracked (not 5); after
  registering the session, a `(d1, seq:5)` delta now applies
- assert(persist): with FLEET_PERSIST set, `hub.db` row count unchanged
- why: admitting the seq for a delta that never landed would silently swallow the run
  when its session finally appears; pins the early-return-before-`reclaim.admit` branch
  in `apply_run_upsert_seq` (`DuplicateDrop` returned but the unknown-session guard runs
  FIRST, so the seq is never committed).
- status: implemented(rust: persist apply_run_upsert_seq unknown-session guard returns
  before admit — covered by the guard's `if session is none` early return); partial(no
  test asserts the seq stays uncommitted after an unknown-session stamped delta);
  TODO(E2E)

---

## End-to-end via a real env (the agent → Hub path that IS wired today)

### L2.HUB.039 — A real `claude -p` run in an env drives its Hub session working→idle/done (rollup observed via fleet CLI)
- layer: L2
- scenarios: [base]
- isolation: shared
- needs: [termSend, hubReachable]
- precondition: env booted, reporter phoned home, `fleet ls --once` shows the env's
  session row `[idle] <env.id>` (session titled `FLEET_SERVER_ID == env.id`)
- action: `termSend` `claude -p "say hi"` into the env terminal (the baked claude
  wrapper installs Fleet hooks → reporter emits Working on UserPromptSubmit/PreToolUse,
  phones home to `ws://HOST:51777`)
- expected: the env's Hub session row goes ACTIVE (`working`/`waiting`) then TERMINATES
  (`idle`/`done`/`dead` — `-p` is one-shot, fires SessionEnd)
- assert: `pollHub(sessionTitle, st => st in {working,waiting})` succeeds, then
  `pollHub(sessionTitle, st => st in {idle,done,dead})` succeeds — both read off
  `fleet ls --once` bracketed state
- why: the full pipeline (in-env claude → hook wrapper → reporter state machine → WS
  phone-home → Hub session registry → merge/rollup → CLI render) must light up; a break
  means Fleet has gone blind to agent activity. SKIPs cleanly if claude is unauth'd or
  the Hub/CLI is absent.
- status: implemented(behaviour `agent.claudeRuns`)

### L2.HUB.040 — A real interactive claude blocked on approval drives the Hub session to `waiting`
- layer: L2
- scenarios: [base]
- isolation: shared
- needs: [termSend, hubReachable]
- precondition: env booted, session registered on the Hub
- action: drive an interactive claude (NOT `-p`) whose prompt triggers a tool-approval,
  OR (the deterministic path) inject a controlled `PreToolUse`-without-`Stop` to the
  real reporter socket so the S16 infer adapter emits `waiting` after its ~1.5 s
  debounce
- expected: the env's Hub session row shows `[waiting]` (the only state that pings)
- assert: `pollHub(sessionTitle, st => st === "waiting")` succeeds reading `fleet ls
  --once`; then the behaviour always tears the prompt down so the env never hangs
- why: waiting is the attention-demanding state the rail badges/pings on — the inferred
  `waiting` path (heuristic, `Confidence::Inferred`) must reach the Hub rollup; uses a
  controlled PreToolUse injection for determinism (overview: determinism over realism).
- status: implemented(behaviour `agent.waitingState`)

### L2.HUB.041 — EDGE: env's session registers on the Hub on boot BEFORE any agent run (empty-but-present session)
- layer: L2
- scenarios: [base]
- isolation: shared
- needs: [hubReachable]
- precondition: env just booted; no claude run started yet
- action: poll the Hub for the env's session title
- expected: the session is present at `[idle]` (the reporter registers on boot,
  runs-empty) — a registered-but-idle session, distinct from "absent"
- assert: `sessionLineFor(hubSnapshot(), env.id)` returns a non-null `[idle]` row with
  zero run activity; if the Hub/CLI is absent the test SKIPs (not fails)
- why: the env must announce itself to the Hub at boot so the rail shows the env even
  before any agent activity — distinguishes "env up, idle" from "env never phoned
  home"; this is the boot-gate the agent behaviours rely on.
- status: implemented(behaviour `agent.claudeRuns` boot/Hub-availability gate —
  `pollHub(...,()=>true)` + `sessionLineFor`)

### L2.HUB.042 — EDGE: no-network env — reporter can't reach the Hub, phone-home FAILS, but the editor stays drivable
- layer: L2
- scenarios: [no-network]
- isolation: fresh
- needs: [hubReachable]
- precondition: env started `--network none`; Hub up on the host (unreachable from the
  container)
- action: boot the env, attempt a claude run
- expected: the env's session NEVER appears in `fleet ls` (reporter retries phone-home
  and fails); bridge commands still work in-env
- assert: `pollHub(env.id, ()=>true, {ms:short})` times out → row absent; meanwhile an
  in-env bridge `executeCommand` still succeeds (editor drivable) — phone-home FAILS
  but commands WORK
- why: Hub unreachability must degrade gracefully — the env keeps working, it just
  doesn't report; guards against the reporter's failure coupling the editor's
  availability.
- status: TODO (no-network scenario exists in scenarios/resourceFailure.mjs but no
  behaviour asserts the Hub-absent + editor-present split)

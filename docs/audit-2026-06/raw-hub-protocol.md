# fleet-hub + fleet-protocol (agent 4) — web-sourced (schemars CHANGELOG, tokio broadcast, sqlite WAL, serde#1189)
- [HIGH concurrency] #1 blocking rusqlite I/O on tokio worker INSIDE global async-Mutex (no spawn_blocking) -> slow disk stalls all reporters+faces. persist.rs:148 under server.rs:75 — M
- [HIGH atomicity] #2 mute/unmute/solo mutate memory THEN persist; append-fail only logged -> memory/log diverge, contradicts server.rs:93-97 doc. persist.rs:452,498 — S-M
- [MED scale] #3 append-only log never compacts; flag flips re-append full snapshots -> unbounded growth + replay. persist.rs:494 — M
- [S deadep] #4 fleet-protocol declares thiserror, never used. protocol Cargo.toml:29 — S
- [S schema] #5 schema hardcodes $schema draft-07 but schemars 1.2 emits 2020-12/$defs. schema.rs:40 — S
- [M serde] #6 Urgency::None rename="null" (string) redundant w/ Option<Urgency>. state.rs:74 — M (wire bump)
- [M dup] #7 delta vocab triplicated: ClientMessage vs PersistEvent vs Event + 2 match ladders. wire/persist/events — M
- [M race] #8 subscribe boundary DOUBLE-delivers (rx attached at accept, snapshot later); doc claims neither lost nor double. server.rs:366 — M
- [S-M bug] #9 empty session keeps stale rollup_state -> stuck unread badge. merge.rs:78 — S-M
- [S perf] #10 focus/dismiss-by-run deep-clones whole state to find one run. server.rs:159 — S
- [S-M fragile] #11 hand-rolled ISO-8601 parser + civil-date math (again). persist.rs:619 — S-M
- [M testsmell] #12 read-only-log mechanism + with_trace wrappers built only for coverage. — M
CLEARED: WS idiom OK; SQLite WAL corruption-safe (only loses last txn); flatten+internally-tagged works (latent).

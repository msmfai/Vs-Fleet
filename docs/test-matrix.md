# VS Fleet seam/runtime test matrix

Tracks the seam/runtime test layers across **every** Tauri command and user-facing
UI flow — generalizing the layers that gate the rename flow (the template) to the
whole frontend↔IPC↔webview boundary.

Layers:
- **B — contract** ✅: `scripts/check-frontend-contract.sh` asserts each `invoke()`
  maps to a registered `#[tauri::command]` with matching argument KEYS
  (snake↔camel), and flags dead commands (no caller, not allow-listed).
- **B — lint** ✅: webview-safety lint bans WKWebView-unsupported / platform-divergent
  web APIs in `ui/` (prompt/alert/confirm/open); clipboard is allowed (guarded +
  execCommand fallback).
- **C — frontend unit**: vitest + jsdom over `ui/main.js` (mocked `window.__TAURI__`);
  asserts the invoke command+args and the observable DOM/state change.
- **E — Rust round-trip**: deterministic integration test asserting observable state —
  `FLEET_PROBE_CONTROL_PORT` probe (`/servers`,`/selected`,…) for state-mutating
  commands; in-process Hub harness asserting the emitted `Command` for Hub-channel
  commands.
- **D — UI E2E**: tauri-driver + WebdriverIO drives the real rail webview (rail-only
  mode), DOM-driven, asserting observable rail DOM.

Legend: ✅ done · 🔸 partial · ⬜ gap · — n/a

## Tauri commands

| Command | UI flow / invoke site | B contract | C vitest | E round-trip | D E2E |
|---|---|---|---|---|---|
| `rename_server` | row menu → Rename (`renameRow`) | ✅ | ✅✅ regressions | ✅ `/rename/<id>` | ✅ `rename.e2e.js` |
| `select_server` | click row / Open (`activateServer`) | ✅ | ✅ | ✅ `/select/<id>` | ⬜ |
| `close_server` | row menu → Close (`closeServer`) | ✅ | ✅ | ✅ `/close/<id>` | ⬜ |
| `get_servers` | `refreshServers` (bootstrap) | ✅ | ✅ render rows | ✅ `/servers` | (indirect) |
| `selected_server` | `refreshServers`/sync | ✅ | ✅ | ✅ `/selected` | (indirect) |
| `spawn_server_with_options` | create menu (home / open-folder) | ✅ | ✅ open-folder spawns | ✅ pure routing (`resolve_spawn_route`) + smoke-only live spawn | ⬜ |
| `open_server_external` | row menu → Open in Browser | ✅ | ✅ `openRowInBrowser` | ✅ pure URL→open (`external_open_command`) + smoke-only live open | ⬜ |
| `set_session_muted` | row menu → Mute (`toggleMuteRow`) | ✅ | ✅ | ✅ Hub harness | ⬜ |
| `set_session_soloed` | row menu → Solo (`toggleSoloRow`) | ✅ | ✅ | ✅ Hub harness | ⬜ |
| `dismiss_session` | row menu → Dismiss (`dismissRow`) | ✅ | ✅ | ✅ Hub harness | ⬜ |
| `focus_session` | `focusSession` | ✅ | ✅ | ✅ Hub harness | ⬜ |
| `get_inbox` | `refreshInbox` (bootstrap) | ✅ | ✅ inbox events | ✅ `/inbox` | — |
| `get_host_status` | `refreshStatus` (bootstrap) | ✅ | ✅ host-status | ✅ `/host-status` | — |
| `clear_host_status_if_current` | `clearStatusOverride` (auto) | ✅ | ✅ | ✅ `/host-status/clear` | — |

(`spawn_server` was removed — it was a registered command with no caller; the
dead-command check now guards against reintroducing one.)

## UI-only flows (no 1:1 command)

| Flow | C vitest | D E2E |
|---|---|---|
| Palette: open / search-filter / choose / jump-next-unread / cycle-unread | ✅ | ⬜ |
| Create menu: open / spawn-home / open-folder (domPrompt) | ✅ | 🔸 (open-folder via E2E owed) |
| Unread jump button enable/disable | ✅ | ⬜ |
| Row context menu render (Open/Rename/Close/…) | ✅ | 🔸 (rename only) |
| Status / error override render + auto-clear | ✅ | — |
| Unread / waiting / attention indicators | ✅ | ⬜ |

## Outstanding work (drives the goal)

1. ✅ **B-contract**: arg-key matching + dead-command detection done; `spawn_server` removed.
2. ✅ **B-lint**: audited; `window.open` added; clipboard documented as allowed.
3. **C**: confirm every enumerated path is covered; raise the per-file vitest threshold
   to the real floor; fill any straggler handler. *(CI-only / node.)*
4. ✅ **E**: every command has a Rust-boundary round-trip (probe / Hub-harness /
   pure-logic+smoke).
5. **D**: extend the tauri-driver suite to spawn-appears, select/switch, open-folder,
   mute/solo visuals, dismiss-removes-row, unread-badge, open-external. *(CI-only / node.)*
6. **F (CI)**: every layer gates PRs (no ratchet-down); existing layers + the 100%
   unit-coverage gate stay green; Node runs CI-only.

Any bug a layer surfaces (as rename did) is fixed + regression-locked in the same pass.

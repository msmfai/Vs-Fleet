# L1.VIEW — Views, Panels, Sidebar, Palette, Quick-Open, Status Bar, Layout, Zen, Zoom

Workbench-chrome surface driven through the bridge `command` action. The hard
constraint that shapes every entry here: **the Snapshot (§3.3) exposes NO chrome
state** — no sidebar/panel visibility, no focused-view id, no zen/zoom level, no
quick-input contents, no status-bar items. So for pure-chrome commands the only
faithful observable is "executeCommand resolved ok" (env.act throws on `!ok`), with
the snapshot fields it *does* expose (`terminalCount`, `visibleEditors`, `openTabs`,
`activeEditor`) captured as **invariants** (must NOT drift) rather than change
targets. Entries that flip something the bridge CAN read (an editor opens, a setting
mutates) make a real assertion and are marked accordingly. Any entry needing a new
observable names the exact Snapshot field a Track-D upgrade must add.

Command-id source of truth: `crates/fleet-host/src/mux.rs` VIEW/GO menus + the
existing behaviours `core.mjs` / `viewsSettings.mjs`.

---

### L1.VIEW.001 — Command Palette opens (showCommands round-trips)
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: workbench booted, ext-host online, no file required
- action: executeCommand "workbench.action.showCommands"
- expected: the command resolves ok (quick-input palette overlay opens; not snapshot-observable)
- assert: env.act returns without throwing (bridge reply `ok:true`); snapshot after has `terminalCount`/`visibleEditors` unchanged vs before (palette is editor-neutral)
- why: cheapest smoke of the whole bridge round-trip (WS → ext-host → command registry → reply); first test to go red if activation/transport regresses. The palette widget itself is a transient overlay the headless snapshot can't see, so asserting on it would be flaky.
- status: implemented (behaviour `palette.open`)

### L1.VIEW.002 — Palette repeat: showCommands twice in a row is idempotent
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: palette already opened once (VIEW.001 ran)
- action: executeCommand "workbench.action.showCommands" a second time with palette already open
- expected: second call resolves ok (re-focuses/re-opens the same quick-input; no error, no second overlay)
- assert: env.act second call returns ok; `openTabs` length unchanged across both calls
- why: EDGE (repeat) — guards that re-issuing a focus/overlay command on an already-open overlay never throws or stacks editors; a common refactor break is double-dispatch raising.
- status: implemented (behaviour `view.paletteRepeat`)

### L1.VIEW.010 — Toggle Primary Side Bar resolves and is editor-neutral
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: side bar visible (default), at least the Explorer view present
- action: executeCommand "workbench.action.toggleSidebarVisibility"
- expected: command resolves ok; side bar hides (visibility NOT in Snapshot)
- assert: env.act ok; snapshot `visibleEditors` identical before vs after (toggling chrome opens/closes no editor)
- machine-state: procs unchanged; mem Δ negligible (pure UI layout)
- why: pins that side-bar toggle is a layout-only op with no editor lifecycle effect; guards the command-dispatch path to stock workbench commands. Real visibility assertion blocked on a Track-D `sideBarVisible` Snapshot field.
- status: implemented (behaviour `view.toggleSidebar`)

### L1.VIEW.011 — Toggle Side Bar twice returns to the original visible state
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: side bar visible
- action: executeCommand "workbench.action.toggleSidebarVisibility" twice
- expected: both calls resolve ok; net visibility back to visible
- assert: both env.act calls ok; `visibleEditors` and `openTabs` unchanged across the pair
- why: EDGE (repeat / round-trip) — a toggle must be its own inverse; guards a state-tracking regression where the second toggle no-ops or errors. Visibility round-trip itself awaits a Track-D field.
- status: implemented (behaviour `view.toggleSidebarRoundtrip`)

### L1.VIEW.020 — Toggle Panel resolves without disposing terminals
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: panel hosts the integrated terminal; ≥0 terminals open
- action: executeCommand "workbench.action.togglePanel"
- expected: command resolves ok; panel hides (visibility NOT in Snapshot) but any terminal process stays alive
- assert: env.act ok; snapshot `terminalCount` identical before vs after (hide ≠ kill)
- machine-state: shell `procs` unchanged (terminals survive a hidden panel)
- why: pins that panel visibility and terminal lifecycle are DECOUPLED — a refactor of terminal disposal that killed shells on panel-hide would drift `terminalCount` and this catches it even before a `panelVisible` Snapshot field exists.
- status: implemented (behaviour `view.togglePanel`)

### L1.VIEW.021 — Toggle Panel with one open terminal keeps the same terminal name
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: exactly one terminal open (created via terminal.new), recorded its `terminals[0]` name
- action: executeCommand "workbench.action.togglePanel" (hide) then again (show)
- expected: both calls ok; the surviving terminal keeps its identity
- assert: snapshot `terminals` array (names) identical before-hide and after-show; `terminalCount` == 1 throughout
- why: EDGE (lifecycle under repeat) — stronger than VIEW.020: not just the count but the named-terminal identity must survive a hide/show, guarding against silent recreate.
- status: implemented (behaviour `view.panelKeepsTerminalIdentity`)

### L1.VIEW.030 — Open Problems view resolves and reports diagnostics count
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: workbench booted; diagnostics count is whatever the scenario produced
- action: executeCommand "workbench.actions.view.problems"
- expected: command resolves ok (Problems view focused; focus NOT in Snapshot)
- assert: env.act ok; record snapshot `diagnostics` as evidence (this command reveals, does not mutate diagnostics)
- why: smoke for the Problems command id + the `diagnostics` Snapshot field plumbing. `diagnostics` is evidence not an assertion target because reveal doesn't change the count. A focused-view assertion needs a Track-D `focusedView` field.
- status: implemented (behaviour `problems.open`)

### L1.VIEW.040 — Show Explorer view focuses the file tree
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: side bar present (may be on any view)
- action: executeCommand "workbench.view.explorer"
- expected: command resolves ok (Explorer becomes the active side-bar view)
- assert: env.act ok; `visibleEditors` unchanged (switching side-bar view opens no editor)
- why: covers the View-menu Explorer id from mux.rs; guards the side-bar view-switch dispatch. Real "Explorer focused" assertion blocked on a Track-D `focusedView` field.
- status: implemented (behaviour `view.showExplorer`)

### L1.VIEW.041 — Show Search view focuses search input
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: side bar present
- action: executeCommand "workbench.view.search"
- expected: command resolves ok (Search view focused)
- assert: env.act ok; `visibleEditors` unchanged
- why: covers the Search side-bar view id; pairs with the 14-search area's find-in-files. Focus assertion awaits Track-D `focusedView`.
- status: implemented (behaviour `view.showSearch`)

### L1.VIEW.042 — Show Source Control view focuses SCM
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: side bar present (repo may or may not be a git repo)
- action: executeCommand "workbench.view.scm"
- expected: command resolves ok (SCM view focused regardless of git state)
- assert: env.act ok; `visibleEditors` unchanged
- why: covers the SCM side-bar id; must resolve even in a non-git workspace (the view shows "no repo" rather than erroring) — that is the EDGE in VIEW.043.
- status: implemented (behaviour `view.showScm`)

### L1.VIEW.043 — Show Extensions view in a no-folder workspace still resolves
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted; (edge) no workspace folder open
- action: executeCommand "workbench.view.extensions"
- expected: command resolves ok even with no folder (Extensions view is workspace-independent)
- assert: env.act ok
- why: EDGE (missing precondition) — view-switch commands must not require a folder; guards a regression where a no-folder boot makes side-bar view ids throw.
- status: implemented (behaviour `view.showExtensions`)

### L1.VIEW.050 — Toggle integrated Terminal via the View menu creates/reveals a terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: zero terminals open (`terminalCount` == 0)
- action: executeCommand "workbench.action.terminal.toggleTerminal"
- expected: command resolves ok AND first toggle creates a terminal (0→1)
- assert: snapshot `terminalCount` delta == +1 on first toggle (from empty, toggle creates one)
- machine-state: `procs` +1..+3 (a shell spawns)
- why: EDGE (empty state) — unlike toggleSidebar/togglePanel, `toggleTerminal` from zero terminals has a snapshot-OBSERVABLE effect (it must spawn one), so this is a real assertion, not a resolve-only smoke.
- status: implemented (behaviour `view.toggleTerminalFromZero`)

### L1.VIEW.051 — Toggle Terminal with one open terminal only hides it (count unchanged)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: exactly one terminal open (`terminalCount` == 1)
- action: executeCommand "workbench.action.terminal.toggleTerminal"
- expected: command resolves ok; the panel hides but the terminal survives
- assert: snapshot `terminalCount` unchanged (== 1) after the toggle
- why: EDGE (non-empty state) — the same command id has DIFFERENT observable behaviour depending on precondition (create-from-zero vs hide-existing); this entry pins the hide branch so the two contracts can't be conflated.
- status: implemented (behaviour `view.toggleTerminalHidesOne`)

### L1.VIEW.060 — Toggle Output panel resolves
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted
- action: executeCommand "workbench.action.output.toggleOutput"
- expected: command resolves ok (Output panel shown/hidden)
- assert: env.act ok; `terminalCount` unchanged (Output is not a terminal)
- why: covers the View-menu Output id; guards that the Output channel surface is reachable. Visibility itself needs a Track-D field.
- status: implemented (behaviour `view.toggleOutput`)

### L1.VIEW.070 — Go to File (quickOpen) command resolves
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: workbench booted, ≥1 file in workspace
- action: executeCommand "workbench.action.quickOpen"
- expected: command resolves ok (quick-open overlay opens; contents NOT in Snapshot)
- assert: env.act ok; `activeEditor` unchanged (opening the picker doesn't switch editors)
- why: covers the Go-menu quickOpen id. NOTE the existing `quickOpen.byName` behaviour deliberately uses the bridge `openFile` action (not synthetic typing into the picker) to actually OPEN a file deterministically — see VIEW.071; this entry is the raw command-dispatch smoke.
- status: implemented (behaviour `view.quickOpenCommand`)

### L1.VIEW.071 — Quick-open-by-name opens a seeded file as the active editor
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [openFile, query]
- precondition: file `fleet-quickopen.txt` seeded at workspace root via `exec printf`
- action: bridge `openFile {path: <root>/fleet-quickopen.txt}` (deterministic substitute for typing into the quickOpen picker, which synthetic-keystroke-into-overlay can't drive headlessly)
- expected: the seeded file becomes the active editor
- assert: snapshot `activeEditor` ends with `fleet-quickopen.txt`; the path appears in `openTabs`
- why: the real "open this named file" outcome — done via `openFile` because driving the quick-input widget by synthetic keystrokes is untrusted/unreliable headlessly; `why` records the determinism-over-realism choice (PLAN §determinism).
- status: implemented (behaviour `quickOpen.byName`)

### L1.VIEW.072 — Go to Line/Column command resolves with an editor open
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, openFile]
- precondition: a multi-line file open and active
- action: executeCommand "workbench.action.gotoLine"
- expected: command resolves ok (line-picker overlay opens)
- assert: env.act ok; `activeEditor` unchanged
- why: covers the Go-menu gotoLine id; the line picker is overlay-only so we assert dispatch. A cursor-line assertion would need the `selection` Snapshot field set after a controlled goto (Track-D).
- status: implemented (behaviour `view.gotoLineWithEditor`)

### L1.VIEW.073 — gotoLine with NO active editor resolves or no-ops cleanly
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: no active editor (all tabs closed)
- action: executeCommand "workbench.action.gotoLine"
- expected: command resolves ok (no editor → picker simply has nothing to navigate; must NOT throw)
- assert: env.act does not throw (`ok:true`)
- why: EDGE (missing precondition) — editor-scoped Go commands must degrade gracefully with no editor; guards a regression where the command rejects instead of no-opping.
- status: implemented (behaviour `view.gotoLineNoEditor`)

### L1.VIEW.080 — Zoom In raises the window zoom level
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench at default zoom
- action: executeCommand "workbench.action.zoomIn"
- expected: command resolves ok (zoom level increments; level NOT in Snapshot today)
- assert: env.act ok. To make this a real assertion, read `window.zoomLevel` via the `setting` query before/after and assert after > before (needs the `setting` cap) — flag a `zoomLevel` Snapshot field as the alternative Track-D observable
- why: covers the View-menu Zoom In id. Honest status: dispatch-only unless the `setting` round-trip is wired; calling it out so the implementer adds the `window.zoomLevel` read rather than a vacuous pass.
- status: TODO (zoom commands not available in code-server web — `workbench.action.zoomIn` throws "command not found"; a test hitting this would fail, so it is left unimplemented)

### L1.VIEW.081 — Zoom Out then Zoom In returns to the original zoom
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, setting]
- precondition: record `window.zoomLevel` via `setting` query
- action: executeCommand "workbench.action.zoomOut" then "workbench.action.zoomIn"
- expected: net zoom level back to the recorded baseline
- assert: `setting {key:"window.zoomLevel"}` after the pair == the recorded baseline value
- why: EDGE (round-trip) — zoom in/out must be inverses; a real config-backed assertion (zoom is persisted to `window.zoomLevel`, unlike per-editor word-wrap) so it is genuinely verifiable.
- status: TODO (zoom commands not available in code-server web — `workbench.action.zoomOut`/`zoomIn` throw "command not found"; a test hitting this would fail, so it is left unimplemented)

### L1.VIEW.090 — New Untitled File adds a tab and an active editor
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: record `openTabs` length N (fresh-window count is env-dependent, so use a delta)
- action: executeCommand "workbench.action.files.newUntitledFile"
- expected: an untitled editor opens and becomes active
- assert: snapshot `openTabs` length delta == +1; `activeEditor` is null-or-untitled (untitled docs have no fsPath) — assert the `openTabs` delta as the stable observable
- why: covers the File-menu New Text File id with a snapshot-OBSERVABLE effect (a tab appears) — a real assertion, using a delta because absolute tab count varies by restored editors/extensions.
- status: implemented (behaviour `view.newUntitledFile`)

### L1.VIEW.091 — Close active editor after newUntitledFile shrinks the tab count
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: one untitled editor open and active (from VIEW.090)
- action: bridge `closeEditor` (→ `workbench.action.closeActiveEditor`)
- expected: the untitled tab closes
- assert: snapshot `openTabs` length delta == -1 vs the post-create count
- why: pairs with VIEW.090 to prove the open/close tab-count round-trip; guards the closeEditor action wiring (which `view.togglePanel`/palette tests don't touch).
- status: implemented (behaviour `view.closeAfterUntitled`)

### L1.VIEW.092 — Close active editor with NO editor open is a clean no-op
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [closeEditor, query]
- precondition: zero open editors (`openTabs` is empty or only non-editor tabs)
- action: bridge `closeEditor`
- expected: resolves ok; nothing to close → no error, count unchanged
- assert: env.request closeEditor returns `ok:true`; `openTabs` length unchanged
- why: EDGE (empty state) — closeActiveEditor on an empty editor area must not throw; guards a regression where the bridge's closeEditor wrapper rejects on no-active-editor.
- status: implemented (behaviour `view.closeEditorNoEditor`)

### L1.VIEW.100 — Toggle Zen Mode resolves
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench in normal (non-zen) layout
- action: executeCommand "workbench.action.toggleZenMode"
- expected: command resolves ok (enters zen — full-screen-ish single-editor chrome; NOT in Snapshot)
- assert: env.act ok; `visibleEditors`/`activeEditor` unchanged (zen hides chrome, not editors)
- why: covers zen layout dispatch; zen is pure chrome so editors must be untouched. Real "in zen" assertion needs a Track-D `zenMode` field.
- status: implemented (behaviour `view.toggleZen`)

### L1.VIEW.101 — Exit Zen Mode (toggle twice) restores normal layout
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: normal layout
- action: executeCommand "workbench.action.toggleZenMode" twice (enter then exit)
- expected: both resolve ok; layout back to normal
- assert: both env.act ok; `visibleEditors`/`openTabs` unchanged across the pair
- why: EDGE (round-trip) — zen toggle must be self-inverse and never drop editors; guards a layout-state regression. Round-trip on a real `zenMode` field awaits Track-D.
- status: implemented (behaviour `view.zenRoundtrip`)

### L1.VIEW.110 — Reset View Locations / Reset Layout resolves from a mutated layout
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: side bar toggled hidden + panel toggled hidden (a non-default layout)
- action: executeCommand "workbench.action.resetViewLocations"
- expected: command resolves ok (default view locations restored)
- assert: env.act ok; subsequent `visibleEditors` unchanged (layout reset opens no editor)
- why: EDGE (recovery) — a reset command must run from an already-mutated layout without error; documents the recovery path. Whether default is actually restored awaits Track-D chrome fields.
- status: implemented (behaviour `view.resetLayout`)

### L1.VIEW.120 — Unknown/invalid command id fails cleanly (does NOT hang)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted
- action: executeCommand "workbench.action.doesNotExist.fleet"
- expected: the bridge replies `{ok:false, error:...}` (VS Code rejects an unregistered id); the harness surfaces it, never hangs
- assert: env.request `{type:"command", id:"workbench.action.doesNotExist.fleet"}` returns `ok:false` with a non-empty `error`; bounded round-trip (no timeout)
- why: EDGE (failure mode) — proves the bridge's error path: an unknown command id must produce a fast `ok:false`, not a silent drop or hang. Guards the `command` handler's `.then(_, reject→fail)` wiring in extension.ts.
- status: implemented (behaviour `view.unknownCommandFails`)

### L1.VIEW.130 — Status bar items exposed via Snapshot (future observable)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [query]
- precondition: workbench booted with default status bar
- action: bridge `query`
- expected: `statusBarItems[]` present in the Snapshot listing the active status-bar entries (e.g. encoding, EOL, indentation, language mode)
- assert: snapshot has a non-empty `statusBarItems` array (once the field ships)
- why: TODO observable — the `_contract.mjs` Snapshot typedef already reserves `statusBarItems` (Track-D/E), but `extension.ts` `snapshot()` does NOT populate it. This entry is the spec for that field so status-bar assertions become possible; flagged as the new cap a Track-E upgrade must add.
- status: TODO

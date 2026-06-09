# L1.SET — Settings: read, write, toggle (per-editor override vs config-backed)

Settings are observed through the bridge `setting {key}` query, which resolves
`vscode.workspace.getConfiguration(section).get(leaf)` — i.e. the **persisted /
effective configuration value**, NOT transient per-editor view state.

> **THE LESSON (load-bearing for this whole area).** `editor.action.toggleWordWrap`
> sets a *per-editor view override* that `config.get` / the `setting` query NEVER
> reflect — a test on it always passes vacuously or always fails, so it is
> fundamentally unverifiable through this path. `editor.action.toggleMinimap`, by
> contrast, mutates the actual `editor.minimap.enabled` **configuration** value,
> which the `setting` query reads back. Every "toggle X" entry below MUST be
> classified config-backed (verifiable via `setting`) vs per-editor-override
> (NOT verifiable via `setting` — needs a different observable or is dispatch-only).

Result-shape note: per §3.3 the `setting` reply may carry the value at top level
(`r.value`) or nested (`r.data.value`); the suite's `settingValue()` helper tolerates
both — entries assert via that helper, not a fixed shape.

---

### L1.SET.001 — Read a default config value (minimap enabled) round-trips
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: workbench booted, no settings mutated
- action: bridge `setting {key:"editor.minimap.enabled"}`
- expected: returns the live boolean default (true on stock VS Code)
- assert: `settingValue(r)` is a boolean (=== true on default image); reply `ok:true`
- why: smoke for the read path — proves `getConfiguration(section).get(leaf)` resolves a known key to a real value, the prerequisite for every toggle assertion below. A `undefined` here means the section/leaf split in extension.ts regressed.
- status: implemented (behaviour `settings.readMinimapDefault`)

### L1.SET.002 — Read an unknown setting key returns undefined, not an error
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: workbench booted
- action: bridge `setting {key:"fleet.nonexistent.setting"}`
- expected: reply `ok:true` with value `undefined` (VS Code returns undefined for unknown keys, not a throw)
- assert: reply `ok:true`; `settingValue(r) === undefined`
- why: EDGE (missing key) — distinguishes "key absent (undefined)" from "query failed (ok:false)". Guards the contract that the read path never throws on an unknown key, so a `undefined` in a toggle test means "not set", not "query broken".
- status: implemented (behaviour `settings.readUnknownKey`)

### L1.SET.003 — Read a setting with an empty key fails cleanly
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: workbench booted
- action: bridge `setting {key:""}`
- expected: reply `ok:false` with error "setting requires key" (extension.ts throws on empty key)
- assert: env.request returns `ok:false`; `error` contains "requires key"
- why: EDGE (bad input) — the handler explicitly guards empty keys; this pins that contract so a future caller gets a clear failure rather than a silent `getConfiguration("").get("")` surprise.
- status: implemented (behaviour `settings.readEmptyKeyFails`)

### L1.SET.010 — Toggle Minimap flips editor.minimap.enabled (config-backed — VERIFIABLE)
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [setting]
- precondition: read baseline `editor.minimap.enabled` (B) via `setting`
- action: executeCommand "editor.action.toggleMinimap"
- expected: the configuration value flips (B → !B)
- assert: re-read `setting {key:"editor.minimap.enabled"}` → `after`; assert `before !== after && after !== undefined`
- why: THE canonical config-backed toggle — the one end-to-end proof that a settings-mutating command's effect is observable through the bridge read path (command write + setting read, full loop). Chosen over word-wrap precisely because minimap mutates real config; if `after===undefined` the read shape regressed, if `before===after` the command stopped mutating config.
- status: implemented (behaviour `settings.toggleMinimap`)

### L1.SET.011 — Toggle Minimap twice returns to the original value (idempotent round-trip)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: read baseline B via `setting`
- action: executeCommand "editor.action.toggleMinimap" twice
- expected: net config value back to B
- assert: `setting {key:"editor.minimap.enabled"}` after the pair === B
- why: EDGE (round-trip) — a boolean toggle must be its own inverse at the config layer; guards a regression where the toggle latches or drifts. Builds directly on the SET.010 read/write loop.
- status: implemented (behaviour `settings.minimapRoundtrip`)

### L1.SET.020 — Toggle Word Wrap is a per-editor override — NOT verifiable via `setting`
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting, openFile]
- precondition: a file open and active; read `editor.wordWrap` baseline via `setting`
- action: executeCommand "editor.action.toggleWordWrap"
- expected: the on-screen wrap toggles BUT `editor.wordWrap` in config is UNCHANGED (it's a transient per-editor view override)
- assert: re-read `setting {key:"editor.wordWrap"}` → assert it is UNCHANGED vs baseline (proving the override does not touch config); pass = "config correctly unchanged"
- why: THE LESSON encoded as a positive test — documents that word-wrap is a per-editor override the `setting` query can't see, so the correct observable is "config did NOT move". Prevents a future contributor from "fixing" the minimap test to use word-wrap and getting a vacuous pass. The real wrap state would need a Track-D editor-view observable.
- status: implemented (behaviour `settings.wordWrapNotConfigBacked`)

### L1.SET.021 — toggleWordWrap with NO active editor resolves or no-ops cleanly
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: no active editor
- action: executeCommand "editor.action.toggleWordWrap"
- expected: resolves ok (no editor → nothing to wrap; must not throw)
- assert: env.act does not throw (`ok:true`)
- why: EDGE (missing precondition) — an editor-scoped setting command must degrade gracefully with no editor; guards a reject-instead-of-noop regression.
- status: implemented (behaviour `settings.toggleWordWrapNoEditor`)

### L1.SET.030 — Toggle Auto Save flips files.autoSave (config-backed — VERIFIABLE)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: read baseline `files.autoSave` via `setting` (string: "off"/"afterDelay"/…)
- action: executeCommand "workbench.action.toggleAutoSave"
- expected: the `files.autoSave` configuration value changes (e.g. "off" ↔ "afterDelay")
- assert: re-read `setting {key:"files.autoSave"}` → `after`; assert `after !== before && after !== undefined`
- why: a second config-backed toggle (covers the File-menu Auto Save id) to prove the read/write loop generalizes beyond booleans to string-valued settings — `files.autoSave` is a string enum, exercising a different value type through the same `setting` path.
- status: implemented (behaviour `settings.toggleAutoSave`)

### L1.SET.031 — Auto Save toggled on then off returns files.autoSave to baseline
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [setting]
- precondition: read baseline `files.autoSave`
- action: executeCommand "workbench.action.toggleAutoSave" twice
- expected: net value back to baseline string
- assert: `setting {key:"files.autoSave"}` after the pair === baseline
- why: EDGE (round-trip on a string enum) — pairs with SET.030; guards that the enum toggle cycles cleanly rather than advancing through states unevenly.
- status: implemented (behaviour `settings.autoSaveRoundtrip`)

### L1.SET.040 — Write a setting via writeFile to settings.json, read it back
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, setting]
- precondition: workspace `.vscode/settings.json` absent or default
- action: bridge `writeFile {path: <root>/.vscode/settings.json, content: '{ "editor.fontSize": 17 }'}` then trigger config reload (or read after VS Code picks up the file change)
- expected: the workspace config now reports the written value
- assert: `setting {key:"editor.fontSize"}` → `settingValue(r) === 17`
- machine-state: one fs change (the settings.json write) visible in `docker diff`
- why: proves the WRITE-via-disk → READ-via-bridge loop (the workspace-settings path, distinct from a command-driven toggle); guards that VS Code's config watcher picks up a bridge-written settings.json. Names the exact key/value so the read-back is unambiguous.
- status: implemented (behaviour `settings.writeSettingsJson`)

### L1.SET.041 — Malformed settings.json does not crash the config read
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, setting]
- precondition: write invalid JSON to `.vscode/settings.json` (e.g. `{ "editor.fontSize": }`)
- action: bridge `writeFile` the malformed content, then bridge `setting {key:"editor.fontSize"}`
- expected: the read still resolves (`ok:true`) returning the prior/default value (VS Code ignores an unparseable settings file, surfaces a Problems entry, does not crash the ext-host)
- assert: reply `ok:true`; `settingValue(r)` is a number or undefined (NOT a thrown bridge error)
- why: EDGE (failure injection) — a corrupt settings file must not take down the `setting` query path; guards ext-host resilience. The diagnostic that VS Code raises could additionally be asserted via the `diagnostics` query (cross-ref 15-diagnostics).
- status: implemented (behaviour `settings.malformedSettingsJson`)

### L1.SET.050 — Workspace-scoped setting overrides the user/default scope
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, setting]
- precondition: default `editor.tabSize` (4); workspace settings.json absent
- action: write `.vscode/settings.json` with `{ "editor.tabSize": 2 }`, then `setting {key:"editor.tabSize"}`
- expected: the effective value is the workspace override (2), not the default (4)
- assert: `settingValue(r) === 2`
- why: proves the `getConfiguration` scope resolution returns the EFFECTIVE (workspace-winning) value, not just the default — the documented behaviour of the `setting` handler's section/leaf split. Guards a regression where the bridge reads only user/default scope.
- status: implemented (behaviour `settings.workspaceOverride`)

### L1.SET.060 — A command-driven toggle and a settings.json write agree on the same key
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [setting, writeFile]
- precondition: write `{ "editor.minimap.enabled": false }` to settings.json; confirm via `setting` it reads false
- action: executeCommand "editor.action.toggleMinimap" (should flip false → true)
- expected: the config value flips relative to the file-written baseline
- assert: `setting {key:"editor.minimap.enabled"}` after toggle === true (i.e. !the written false)
- why: EDGE (interaction) — proves the command toggle and the file write target the SAME config key and compose predictably; guards against the toggle writing a different scope than the file (which would make them silently diverge).
- status: implemented (behaviour `settings.toggleAndWriteAgree`)

### L1.SET.070 — Reading a setting before any editor opens still resolves (config is global)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [setting]
- precondition: workbench booted, ZERO editors open
- action: bridge `setting {key:"editor.minimap.enabled"}`
- expected: resolves ok with the config value (config does not depend on an open editor)
- assert: reply `ok:true`; `settingValue(r)` is a boolean
- why: EDGE (empty editor state) — `getConfiguration` is workspace/global, not editor-scoped, so the read must work with no editor; distinguishes config reads (always available) from editor-view state (needs an editor). Complements the per-editor-override lesson in SET.020.
- status: implemented (behaviour `settings.readBeforeEditor`)

# 14 — Search (find/replace in files + in editor, regex, symbol/quick search)

L1 in-env area. Covers the Search viewlet (`workbench.action.findInFiles` /
`workbench.action.replaceInFiles`), the editor find/replace widget (`actions.find` /
`editor.action.startFindReplaceAction`), Go-to-File quick open
(`workbench.action.quickOpen`), Go-to-Symbol in editor/workspace
(`workbench.action.gotoSymbol` / `workbench.action.showAllSymbols`), and Go-to-Line
(`workbench.action.gotoLine`). Command ids are taken verbatim from
`crates/fleet-host/src/mux.rs` (EDIT/VIEW/GO catalogs).

Project root inside the env is `/home/coder/project` (PROJECT). The bridge snapshot
(`{type:"query"} → data`) exposes `terminals, terminalCount, activeEditor,
visibleEditors, openTabs, diagnostics, editorText?, selection?`; there is NO snapshot
field for the focused viewlet, the Search results, the find-widget match count, or the
quick-open list, so headless search assertions assert the *outcome a search produces*
(active editor / file content on disk / selection range) rather than the widget UI.
Where a behaviour can only confirm the command dispatched, that is stated explicitly
and `expected` names that exact weaker observable — never "verify it works".

Reusable caps: `command, query, openFile, typeText, writeFile, saveAll, fileContent,
editorText, selection`.

---

### L1.SEARCH.001 — Find in Files command reveals the Search viewlet
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command, query]
- precondition: workbench booted, ext-host online, any view focused
- action: executeCommand "workbench.action.findInFiles"
- expected: command resolves ok (bridge reply `ok:true`); no editor/terminal state mutated (terminalCount, openTabs unchanged from before)
- assert: `env.act("workbench.action.findInFiles")` returns without throw (bridge throws on `ok:false`); snapshot.terminalCount and snapshot.openTabs equal before-snapshot values
- why: guards the Search feature's entry-point command id + dispatch. The snapshot cannot see the focused viewlet, so the honest observable is "the canonical reveal-search command exists and ran without side effects"; if a refactor renames/unregisters it or breaks bridge dispatch, env.act throws.
- status: implemented (behaviour `search.findInFiles`)

### L1.SEARCH.002 — Find in Files is idempotent when the viewlet is already open
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, query]
- precondition: `workbench.action.findInFiles` already executed once this session
- action: executeCommand "workbench.action.findInFiles" a second time
- expected: second call also resolves ok; still no editor/terminal state change (snapshot identical to after the first call)
- assert: two consecutive `env.act` calls both return without throw; snapshot deep-equals between the two afters on {terminalCount, openTabs, activeEditor}
- why: EDGE (repeat) — re-revealing an already-open viewlet must be a no-op focus, not an error or a duplicate-panel spawn; catches a regression where re-entry toggles/closes the view.
- status: TODO

### L1.SEARCH.003 — Replace-all rewrites every match in a seeded file on disk
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [writeFile, fileContent]
- precondition: `${PROJECT}/replace-target.txt` seeded with `alpha FINDME beta\nFINDME gamma\nno match here\n` (2 FINDME, 1 non-match line)
- action: rewrite file with `original.replaceAll("FINDME","REPLACED")` via `{type:"writeFile"}` (the deterministic headless equivalent of Search→Replace-All; the viewlet replace UI is non-deterministic headless)
- expected: post content contains "REPLACED", contains NO "FINDME", still contains "no match here"
- assert: `fileContent({path})` after-text passes all three substring checks; evidence.seededOk == (fileContent before == original)
- why: the observable contract of replace-all is "file content on disk reflects the replacement". Three-part check encodes correctness: all matches changed AND non-matching lines preserved; also guards the writeFile→fileContent round-trip every mutating search test builds on.
- status: implemented (behaviour `search.replaceAll`)

### L1.SEARCH.004 — Replace-all on a file with zero matches leaves content byte-identical
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [writeFile, fileContent]
- precondition: `${PROJECT}/no-match.txt` seeded with `nothing to find here\nstill nothing\n`
- action: writeFile the result of `original.replaceAll("FINDME","REPLACED")` (a no-op replacement) then re-read
- expected: after content == original exactly (no spurious mutation, no trailing-newline drift)
- assert: `fileContent({path})` after-text === original string (strict equality)
- why: EDGE (empty result set) — a replace with no matches must not touch the file; guards against a replace path that rewrites/re-encodes even when nothing matched.
- status: TODO

### L1.SEARCH.005 — Replace in Files command resolves (multi-file replace entry-point)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted
- action: executeCommand "workbench.action.replaceInFiles"
- expected: command resolves ok; reveals the Search viewlet in replace mode (UI not snapshot-observable → assert dispatch only)
- assert: `env.act("workbench.action.replaceInFiles")` returns without throw
- why: guards the Replace-in-Files command id from the EDIT menu catalog. Honest observable: the command exists and dispatches; the replace UI itself is not in the snapshot, so SEARCH.003 covers the disk-level effect.
- status: TODO

### L1.SEARCH.006 — Editor Find widget opens via actions.find
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, openFile, writeFile]
- precondition: `${PROJECT}/find-here.txt` written with `needle in haystack\n`, opened so it is the activeEditor
- action: executeCommand "actions.find"
- expected: command resolves ok; activeEditor unchanged (find widget overlays the same editor, does not switch tabs)
- assert: `env.act("actions.find")` no throw; snapshot.activeEditor after == before
- why: guards the in-editor Find command id (`actions.find`, the EDIT-menu "Find"). The find widget is not snapshot-observable; the honest observable is dispatch + that the active editor is not navigated away.
- status: TODO

### L1.SEARCH.007 — Editor Find on no active editor still resolves (no editor open)
- layer: L1
- scenarios: [base, no-folder]
- isolation: fresh
- needs: [command, query]
- precondition: all editors closed (snapshot.activeEditor == null, openTabs empty)
- action: executeCommand "actions.find"
- expected: command resolves ok (VS Code no-ops the find widget with no editor) — NOT an error
- assert: `env.act("actions.find")` returns without throw; snapshot.activeEditor still null afterward
- why: EDGE (missing precondition) — invoking editor-find with no editor must be a clean no-op, not a thrown error that env.act would surface; guards the host's null-editor guard.
- status: TODO

### L1.SEARCH.008 — Start Find-and-Replace widget opens via editor.action.startFindReplaceAction
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, openFile, writeFile]
- precondition: `${PROJECT}/replace-here.txt` written with `aaa bbb aaa\n`, opened as activeEditor
- action: executeCommand "editor.action.startFindReplaceAction"
- expected: command resolves ok; activeEditor unchanged (the find/replace widget overlays the same editor)
- assert: `env.act("editor.action.startFindReplaceAction")` no throw; snapshot.activeEditor after == before
- why: guards the EDIT-menu "Replace" command id. Widget state not snapshot-observable; honest observable is dispatch + no tab navigation.
- status: TODO

### L1.SEARCH.009 — In-editor replace effected via editor mutation reflects in editorText + on disk
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile, saveAll, editorText, fileContent]
- precondition: `${PROJECT}/inplace.txt` written `xxYxx Y end\n` and opened as activeEditor
- action: rewrite the open buffer's content replacing "Y"→"Z" (writeFile to disk + reopen, the deterministic equivalent of using the editor replace widget), saveAll
- expected: snapshot.editorText contains "Z" and no "Y"; on-disk `fileContent` matches the editorText
- assert: snapshot.editorText (after reopen) passes substring checks; `fileContent({path})` === editorText (buffer == disk after save)
- why: proves an editor-level replace produces a consistent buffer AND disk state (no dirty/disk divergence); guards editorText↔fileContent↔saveAll coherence for the find/replace surface.
- status: partial(editorText/selection snapshot fields land via bridge but no behaviour drives an in-editor replace yet)

### L1.SEARCH.010 — Quick Open by exact name makes that file the active editor
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [openFile, query]
- precondition: `${PROJECT}/fleet-quickopen.txt` seeded via `exec printf 'quick open target\n'`; the file is NOT currently the active editor
- action: `{type:"openFile", path}` (the headless equivalent of Quick Open → pick → enter; openFile is the navigation outcome the widget produces)
- expected: snapshot.activeEditor resolves to that file (basename-tolerant match)
- assert: `isActive(snapshot, path)` true after openFile (basename of snapshot.activeEditor == basename of path)
- why: the navigation/resolution contract — a file identified by name becomes the focused active editor, not opened in background/wrong group/preview. Guards openFile focus mechanics that Quick Open relies on.
- status: implemented (behaviour `quickOpen.byName`)

### L1.SEARCH.011 — Quick Open widget command resolves (Go to File…)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted
- action: executeCommand "workbench.action.quickOpen"
- expected: command resolves ok (opens the quick-open input); activeEditor unchanged (no file picked yet)
- assert: `env.act("workbench.action.quickOpen")` no throw; snapshot.activeEditor after == before
- why: guards the GO-menu "Go to File…" command id. The quick-open list is not snapshot-observable, so SEARCH.010 covers the pick outcome and this covers the widget command dispatch.
- status: TODO

### L1.SEARCH.012 — Quick Open of a non-existent name leaves the active editor unchanged
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, query]
- precondition: an editor for `${PROJECT}/anchor.txt` open and active
- action: `{type:"openFile", path:"${PROJECT}/does-not-exist-9f3.txt"}`
- expected: openFile reply is `ok:false` (path missing) OR opens an empty untitled-style doc; in either case the prior anchor.txt remains resolvable and no crash
- assert: catch any throw from `env.request`; assert env still responds to a follow-up `{type:"query"}` (snapshot returns within timeout) — the env did not wedge
- why: EDGE (failure / missing target) — resolving a name that maps to no file must fail cleanly or open empty, never wedge the ext-host; guards openFile error handling.
- status: TODO

### L1.SEARCH.013 — Go to Symbol in Editor command resolves on a populated file
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, openFile, writeFile]
- precondition: `${PROJECT}/syms.txt` written with several lines, opened as activeEditor
- action: executeCommand "workbench.action.gotoSymbol"
- expected: command resolves ok (opens the @-symbol quick pick); activeEditor unchanged
- assert: `env.act("workbench.action.gotoSymbol")` no throw; snapshot.activeEditor after == before
- why: guards the GO-menu "Go to Symbol in Editor…" id. On a plaintext file with no language server the symbol list is empty but the command still dispatches; the honest observable is dispatch + no navigation. Real symbol population needs a +lang image (see 15-diagnostics).
- status: TODO

### L1.SEARCH.014 — Go to Symbol in Workspace command resolves
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: workbench booted (workspace folder open)
- action: executeCommand "workbench.action.showAllSymbols"
- expected: command resolves ok (opens the #-symbol workspace quick pick); no editor state change
- assert: `env.act("workbench.action.showAllSymbols")` no throw; snapshot.activeEditor + openTabs unchanged
- why: guards the GO-menu "Go to Symbol in Workspace…" id. Without a language server the workspace-symbol provider returns nothing; honest observable is dispatch. Populated results require +lang.
- status: TODO

### L1.SEARCH.015 — Go to Symbol in Workspace with no folder open resolves cleanly
- layer: L1
- scenarios: [no-folder]
- isolation: fresh
- needs: [command, query]
- precondition: env booted with no `?folder` → no workspace folder
- action: executeCommand "workbench.action.showAllSymbols"
- expected: command resolves ok (empty result set, no folder to index) — not an error
- assert: `env.act` no throw; follow-up `{type:"query"}` still returns a snapshot (env responsive)
- why: EDGE (empty state / missing workspace) — workspace-symbol search with no folder must no-op, not throw; guards the no-folder scenario's command surface.
- status: TODO

### L1.SEARCH.016 — Go to Line/Column navigates and updates the selection
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, openFile, writeFile, selection]
- precondition: `${PROJECT}/lines.txt` written with 10 numbered lines, opened as activeEditor; cursor at line 0
- action: executeCommand "workbench.action.gotoLine" then move cursor (line 5) — assert via selection snapshot field (headless: command opens the widget; the navigation outcome is the observable)
- expected: snapshot.selection.start.line reflects the navigated line (>0, moved from line 0)
- assert: snapshot.selection.start.line after != snapshot.selection.start.line before (cursor moved to the target line)
- why: guards the GO-menu "Go to Line/Column…" id and that navigation actually moves the cursor (observable via the `selection` snapshot field, the only line-position signal). Names the exact snapshot field rather than the unobservable widget.
- status: partial(selection field shipped in bridge caps; no behaviour drives gotoLine + asserts selection delta yet)

### L1.SEARCH.017 — Find with regex pattern: replace-all using a regex rewrites matching lines
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [writeFile, fileContent]
- precondition: `${PROJECT}/regex-target.txt` seeded `id=001\nid=042\nname=foo\nid=999\n`
- action: writeFile the result of `original.replace(/id=\d+/g, "id=REDACTED")` (the deterministic headless equivalent of a regex Replace-All), then re-read
- expected: every `id=<digits>` line becomes `id=REDACTED`; the `name=foo` line is untouched; no `id=0|4|9...` digit sequences remain on id lines
- assert: `fileContent({path})` after-text matches `/id=REDACTED/` on 3 lines, `!/id=\d/`, and still contains "name=foo"
- why: regex search is the highest-value search mode; the observable is the on-disk effect of the pattern. Guards that a regex replace touches exactly the matching tokens and preserves non-matching lines — the headless analog of toggling the `.*` regex button in the find widget.
- status: TODO

### L1.SEARCH.018 — Case-sensitive replace only rewrites exact-case matches
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [writeFile, fileContent]
- precondition: `${PROJECT}/case-target.txt` seeded `Foo foo FOO\n`
- action: writeFile the result of `original.replace(/\bfoo\b/g, "bar")` (case-sensitive replace; only the lowercase token matches), re-read
- expected: result is `Foo bar FOO\n` — only the lowercase `foo` changed; `Foo` and `FOO` preserved
- assert: `fileContent({path})` after-text === `Foo bar FOO\n` (strict)
- why: EDGE (case-sensitivity toggle) — encodes that the case-sensitive find mode does NOT touch differing-case tokens; guards against a replace that case-folds and over-replaces.
- status: TODO

### L1.SEARCH.019 — Whole-word match does not replace substrings
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [writeFile, fileContent]
- precondition: `${PROJECT}/word-target.txt` seeded `cat category scatter cat\n`
- action: writeFile the result of `original.replace(/\bcat\b/g, "dog")` (whole-word replace), re-read
- expected: result is `dog category scatter dog\n` — only standalone `cat` replaced; `category`/`scatter` untouched
- assert: `fileContent({path})` after-text === `dog category scatter dog\n` (strict)
- why: EDGE (whole-word toggle) — encodes that whole-word find ignores substring occurrences; guards against the find widget's whole-word boundary handling regressing into substring replacement.
- status: TODO

### L1.SEARCH.020 — Add Next Occurrence (multi-cursor find) command resolves
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, openFile, writeFile]
- precondition: `${PROJECT}/multi.txt` written `tok\ntok\ntok\n`, opened as activeEditor with a word selected (or cursor in first `tok`)
- action: executeCommand "editor.action.addSelectionToNextFindMatch"
- expected: command resolves ok (adds a cursor at the next matching occurrence)
- assert: `env.act("editor.action.addSelectionToNextFindMatch")` no throw
- why: guards the SELECTION-menu "Add Next Occurrence" id — the keyboard-driven find-based multi-cursor. Multi-cursor count is not in the snapshot, so the honest observable is dispatch; pairs with a future selection-aware assertion when the snapshot exposes cursor count.
- status: TODO

### L1.SEARCH.021 — Find in Files across a many-files workspace stays responsive
- layer: L1
- scenarios: [many-files]
- isolation: shared
- needs: [command, query]
- precondition: many-files scenario active (~5000 files across 50 dirs)
- action: executeCommand "workbench.action.findInFiles"
- expected: command resolves ok within the bridge timeout; a follow-up `{type:"query"}` returns a snapshot (search indexing does not wedge the ext-host)
- assert: `env.act` returns within timeout (no throw, no harness hang); subsequent snapshot returns
- machine-state: mem Δ during index < a scenario-declared ceiling; no orphan ripgrep procs left after (procs return to baseline)
- why: EDGE (scale) — opening search over a large tree must not hang the bridge or leak the search backend; guards the file-watcher/search path under load on the many-files scenario.
- status: TODO

### L1.SEARCH.022 — Concurrent Find-in-Files + Quick Open do not corrupt each other
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, openFile, query]
- precondition: a seeded file exists and is openable by name
- action: dispatch `workbench.action.findInFiles` and `{type:"openFile", path}` back-to-back (no wait between) — two distinct reqIds in flight
- expected: both replies arrive with matching reqIds; the openFile target becomes activeEditor (the navigation still wins); no reply cross-talk
- assert: both `env.request` promises resolve with their own reqId; final snapshot.activeEditor == the opened file
- why: EDGE (concurrent) — the bridge correlates replies by reqId; guards against two in-flight frames swapping results or one wedging the other. Names the exact correlation mechanism (reqId) and the surviving observable (activeEditor).
- status: TODO

---

Traceability: SEARCH.001 ← `search.findInFiles`; SEARCH.003 ← `search.replaceAll`;
SEARCH.010 ← `quickOpen.byName`. SEARCH.009 / SEARCH.016 are partial (snapshot fields
`editorText`/`selection` exist as bridge caps but no behaviour drives the search action
+ asserts the delta yet). All others TODO.

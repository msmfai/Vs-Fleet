# L1.EDITOR — Editors: open / close / split / tabs / save / dirty / diff / peek / format / navigation

In-env (L1) coverage of the VS Code editor surface, driven through the bridge
(`command` / `openFile` / `writeFile` / `typeText` / `saveAll` / `closeEditor`) and
asserted on the bridge `query` Snapshot fields (`activeEditor`, `visibleEditors[]`,
`openTabs[]`, `editorText`, `selection`, `diagnostics`) or out-of-band via
`exec` (`docker exec cat`/`test`). Workspace root inside the env is
`/home/coder/project` (`PROJECT`). All command ids below are the literal VS Code
command ids the bridge runs via `vscode.commands.executeCommand`.

Conventions: caps gate via `needs`; absent cap → clean SKIP. `editorText`/`selection`
are advertised caps (bridge `CAPS`) but are Track-D/E Snapshot extensions — entries
that assert on them mark the dependency. `exec` reads bypass the editor and prove
durability/the filesystem truth.

---

### L1.EDITOR.001 — openFile makes a written file the active editor
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [writeFile, openFile, fileContent]
- precondition: `PROJECT/fleet-create.txt` does not exist; active editor is the Welcome tab (or none)
- action: request `writeFile {path:PROJECT/fleet-create.txt, content:"FLEET_CREATE_OK\nline two\n"}` then request `openFile {path}`
- expected: the named file becomes the active editor of the active group
- assert: `snapshot.activeEditor` basename == `fleet-create.txt` (basename-tolerant); AND `fileContent {path}` reply `.text` (or `.data.text`) includes `"FLEET_CREATE_OK"`
- machine-state: fsChanges +1 (the new file)
- why: foundational write→open→read loop; the activeEditor half guards openFile focus, the fileContent half guards that writeFile flushed real bytes to disk (not an unsaved buffer)
- status: implemented (behaviour `file.create`)

### L1.EDITOR.002 — openFile on a missing path does not focus a phantom editor
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile]
- precondition: `PROJECT/does-not-exist.txt` is absent (`exec test -f` → false)
- action: request `openFile {path:PROJECT/does-not-exist.txt}`
- expected: either the bridge reply `ok:false`, OR `activeEditor` is unchanged from `before` (no phantom file opened)
- assert: `before.activeEditor === after.activeEditor` OR reply `ok:false`; `exec test -f PROJECT/does-not-exist.txt` still false (open didn't create it)
- edges: empty-precondition edge of L1.EDITOR.001 (target file absent)
- why: openFile must not silently materialise a nonexistent file as the active editor nor create it on disk
- status: implemented (editor.openMissingNoop)

### L1.EDITOR.003 — Close the active (Welcome) tab shrinks openTabs by one
- layer: L1
- scenarios: [base, small-repo]
- precondition: fresh workbench with the Welcome/Get-Started tab open; `openTabs.length == N (N≥1)`
- action: executeCommand `workbench.action.closeActiveEditor`
- expected: `openTabs.length` strictly decreases (N → N-1)
- assert: `after.openTabs.length < before.openTabs.length`; if `openTabs` not exposed → record "not measurable" (pass=false, honest)
- why: proves the command channel round-trips a genuine workbench mutation AND openTabs reflects editor lifecycle; cheapest tripwire for the observe→act→observe loop
- status: implemented (behaviour `file.openWelcomeClose`)

### L1.EDITOR.004 — closeActiveEditor with NO editor open is a no-op, not an error
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [closeEditor]
- precondition: every editor closed first (repeat `workbench.action.closeAllEditors` until `openTabs.length == 0`)
- action: executeCommand `workbench.action.closeActiveEditor`
- expected: command returns ok (or closeEditor reply `ok:true`); `openTabs.length` stays 0
- assert: `after.openTabs.length === 0`; the command did not throw / reply ok
- edges: empty-state edge of L1.EDITOR.003 (close with nothing to close)
- why: closing with no active editor must no-op cleanly; guards against a refactor that errors or wedges when the group is empty
- status: TODO

### L1.EDITOR.005 — closeAllEditors empties openTabs
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: open ≥2 files (`a.txt`, `b.txt`) so `openTabs.length ≥ 2`
- action: executeCommand `workbench.action.closeAllEditors`
- expected: `openTabs.length` → 0 and `activeEditor` → null
- assert: `after.openTabs.length === 0`; `after.activeEditor == null`
- why: bulk-close path distinct from single close; guards group teardown reflecting in the snapshot
- status: implemented (editor.closeAllEmpties)

### L1.EDITOR.006 — Split editor right yields two visible editors
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: `PROJECT/fleet-split.txt` written and opened so there is one active editor; `visibleEditors.length == V`
- action: executeCommand `workbench.action.splitEditor`
- expected: `visibleEditors.length` strictly grows (V → V+1) — the doc is cloned into a new group beside the current
- assert: `after.visibleEditors.length > before.visibleEditors.length`; if not exposed → "not measurable"
- why: guards the tabs (logical docs) vs editor-groups/visibleEditors (on-screen panes) distinction — split adds a group, not a tab; a tab-count proxy would not change
- status: implemented (behaviour `editor.splitRight`)

### L1.EDITOR.007 — Split editor with NO active editor is a no-op (visibleEditors unchanged)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [closeEditor]
- precondition: all editors closed; `visibleEditors.length == 0`
- action: executeCommand `workbench.action.splitEditor`
- expected: nothing to clone → `visibleEditors.length` stays 0 (no error)
- assert: `after.visibleEditors.length === before.visibleEditors.length` (==0); command did not throw
- edges: empty-state edge of L1.EDITOR.006
- why: split on an empty workbench must no-op deterministically; documents the precondition that L1.EDITOR.006 first opens a file
- status: TODO

### L1.EDITOR.008 — splitEditorDown stacks a second pane below
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: one file open; `visibleEditors.length == 1`
- action: executeCommand `workbench.action.splitEditorDown`
- expected: `visibleEditors.length` → 2 (vertical split)
- assert: `after.visibleEditors.length === before.visibleEditors.length + 1`
- why: distinct orientation command from `splitEditor`; both must register a new visible group
- status: implemented (editor.splitDown)

### L1.EDITOR.009 — Next/previous editor cycles the active editor among open tabs
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: two files `a.txt`, `b.txt` open in one group; `activeEditor` basename == `b.txt`
- action: executeCommand `workbench.action.nextEditor` then `workbench.action.previousEditor`
- expected: nextEditor moves `activeEditor` to a different tab; previousEditor returns it
- assert: after nextEditor, `activeEditor` basename != `b.txt`; after previousEditor, `activeEditor` basename == `b.txt`
- why: tab navigation must change the snapshot's activeEditor deterministically; guards the cycle wiring
- status: implemented (editor.nextPrevCycles)

### L1.EDITOR.010 — Reopen closed editor restores the last-closed tab
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: open `a.txt`, capture `openTabs`, close it with `workbench.action.closeActiveEditor` (tab gone)
- action: executeCommand `workbench.action.reopenClosedEditor`
- expected: `a.txt` reappears in `openTabs` and becomes `activeEditor`
- assert: `after.openTabs` references basename `a.txt`; `after.activeEditor` basename == `a.txt`
- edges: repeat — reopen when nothing was closed → no-op, no error
- why: guards the closed-editor history stack and that reopen re-focuses; subtle state most refactors don't test
- status: TODO

### L1.EDITOR.011 — Type into the active editor + saveAll persists to disk
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [typeText, saveAll, writeFile, openFile]
- precondition: `PROJECT/fleet-save.txt` written EMPTY and opened; `exec cat` of it == "" (clean before)
- action: request `typeText {text:"FLEET_SAVED_MARKER"}` then request `saveAll {}`
- expected: the on-disk bytes (read out-of-band) now contain the typed marker
- assert: `exec cat PROJECT/fleet-save.txt` includes `"FLEET_SAVED_MARKER"` (the read goes through the container shell, NOT the bridge, to prove the persistence path)
- why: the dirty→save→persist contract — the difference between an editor that looks saved and bytes that hit disk; out-of-band read isolates type-vs-save break
- status: implemented (behaviour `editor.saveDirty`)

### L1.EDITOR.012 — A typed-but-unsaved editor leaves disk unchanged (dirty state)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [typeText, writeFile, openFile, fileContent]
- precondition: `PROJECT/fleet-dirty.txt` written EMPTY and opened
- action: request `typeText {text:"DIRTY_ONLY"}` and do NOT save
- expected: the buffer (fileContent prefers open-doc text) shows `DIRTY_ONLY` but disk does not
- assert: `fileContent {path}` `.text` includes `DIRTY_ONLY`; AND `exec cat PROJECT/fleet-dirty.txt` does NOT include `DIRTY_ONLY` (still empty)
- edges: the un-saved counterpart of L1.EDITOR.011 — proves typeText alone never touches disk
- why: distinguishes buffer mutation from persistence; if fileContent ever started reading disk for open docs (or saveAll fired implicitly) this catches it
- status: partial(disk-vs-buffer split asserted nowhere; `editor.saveDirty` only asserts the saved case)

### L1.EDITOR.013 — Revert active file discards unsaved edits
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [typeText, writeFile, openFile, fileContent]
- precondition: `PROJECT/fleet-revert.txt` written `"original\n"` and opened; type `"GARBAGE"` (now dirty)
- action: executeCommand `workbench.action.files.revert`
- expected: the editor buffer returns to `"original\n"`; `GARBAGE` is gone
- assert: `fileContent {path}` `.text` == `"original\n"` (or includes `original` and excludes `GARBAGE`)
- edges: revert a clean (non-dirty) file → no-op, buffer unchanged
- why: guards the revert command actually reloading from disk and discarding in-memory edits
- status: TODO

### L1.EDITOR.014 — saveAll with no dirty editors is a clean no-op
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [saveAll, writeFile, openFile]
- precondition: open a file, ensure not dirty (no typeText since open)
- action: request `saveAll {}`
- expected: reply `ok:true, saved:true`; disk bytes unchanged
- assert: reply `ok:true`; `exec cat` of the file identical to its pre-saveAll bytes
- edges: empty-state edge of L1.EDITOR.011 (nothing dirty to save)
- why: saveAll must succeed and not corrupt/rewrite clean files when there is nothing to flush
- status: TODO

### L1.EDITOR.015 — New untitled file opens as an unsaved editor
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [closeEditor]
- precondition: `openTabs.length == N`
- action: executeCommand `workbench.action.files.newUntitledFile`
- expected: an untitled editor opens; `openTabs.length` → N+1 and `activeEditor` references an `Untitled` doc (scheme `untitled:`)
- assert: `after.openTabs.length === before.openTabs.length + 1`; `after.activeEditor` matches `/^Untitled|untitled:/` (label or path)
- edges: repeat → each call adds another `Untitled-N`
- why: untitled docs have no on-disk path; guards the snapshot representing scheme-only editors and the count delta
- status: implemented (editor.newUntitled)

### L1.EDITOR.016 — Diff editor opens for a modified tracked file
- layer: L1
- scenarios: [small-repo]
- isolation: fresh
- needs: [writeFile, openFile]
- precondition: a git repo (`small-repo` scenario) with tracked `README` committed; modify it via `exec` so SCM sees one change
- action: executeCommand `workbench.action.compareEditorWith` (or open the SCM diff for the changed resource)
- expected: a diff editor opens showing the working-tree change; `visibleEditors`/`openTabs` references a diff (`↔`/`Working Tree` label)
- assert: `after.openTabs` (or `activeEditor`) contains a diff-titled entry (matches `↔|Working Tree|Index`); `exec git status --porcelain` shows the file as `M`
- why: guards diff-editor surfacing for SCM changes — the inline-review path agents/users rely on
- status: TODO

### L1.EDITOR.017 — Open Changes (SCM diff) for a dirty tracked file shows the delta
- layer: L1
- scenarios: [small-repo]
- isolation: fresh
- needs: [writeFile]
- precondition: tracked file committed; append a line via `exec`
- action: executeCommand `workbench.view.scm` then open the resource's "Open Changes"
- expected: the diff editor's left==HEAD, right==working tree with the appended line
- assert: `exec git diff -- <file>` contains the appended marker (out-of-band truth); a diff editor is visible (`openTabs` diff entry)
- edges: open changes on an UNmodified tracked file → no diff content (empty diff), no error
- why: ties SCM decoration to the diff view; the appended-line marker is the observable, asserted via git
- status: TODO

### L1.EDITOR.018 — Go to Definition / Reveal Definition navigates within a file (needs +lang)
- layer: L1
- scenarios: [python, node]
- isolation: fresh
- needs: [openFile, writeFile, selection]
- precondition: language scenario (e.g. `python`) with a file defining `foo` and a call site `foo()`; cursor placed on the call site
- action: executeCommand `editor.action.revealDefinition`
- expected: the active editor's selection/cursor jumps to the `def foo` line
- assert: `after.selection.start.line` == the definition line (lower than the call-site line); `activeEditor` unchanged (same-file def)
- edges: reveal definition with NO language server (`base` scenario) → no-op, selection unchanged, no error
- why: guards language-server-backed navigation surfacing in the `selection` Snapshot field; gated on +lang image
- status: TODO

### L1.EDITOR.019 — Go to References / peek references lists usages (needs +lang)
- layer: L1
- scenarios: [python, node]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: language scenario with a symbol used in ≥2 places; cursor on a usage
- action: executeCommand `editor.action.goToReferences`
- expected: a peek/references view opens (transient overlay) — assert the navigation outcome, not the widget
- assert: best-effort — `activeEditor` stays the symbol's file; the command returns ok (peek is a transient overlay the headless snapshot does not reliably expose, like palette)
- edges: goToReferences on a symbol with zero references → command ok, no peek content
- why: guards the references command running on a +lang host; honest about peek being an unobservable overlay (assert ok + no crash)
- status: TODO

### L1.EDITOR.020 — Format Document rewrites buffer to formatter output (needs +lang)
- layer: L1
- scenarios: [python, node]
- isolation: fresh
- needs: [writeFile, openFile, fileContent, saveAll]
- precondition: a file written with deliberately bad formatting (e.g. JS `const x ={a:1,b:2}` with no spaces); a formatter available for that language
- action: executeCommand `editor.action.formatDocument` then `saveAll`
- expected: the buffer/disk reflows to formatter output (spaces around `:`/`,`, consistent quotes)
- assert: `fileContent {path}` `.text` differs from the input AND matches the formatter's canonical spacing (e.g. includes `a: 1`); `exec cat` confirms persistence
- edges: format a file whose language has NO formatter (`base` plain .txt) → buffer unchanged, no error
- why: guards the format command actually invoking a formatter and mutating the document; +lang-gated
- status: TODO

### L1.EDITOR.021 — Go to Line moves the cursor to a target line
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile, selection]
- precondition: a 10-line file open, cursor at line 0
- action: executeCommand `workbench.action.gotoLine` with arg `{lineNumber:5}` (or drive via the quick-input)
- expected: cursor lands on line 5
- assert: `after.selection.start.line == 4` (0-indexed) i.e. line 5 1-indexed
- edges: gotoLine beyond EOF (line 999 in a 10-line file) → cursor clamps to last line, no error
- why: guards line navigation reflecting in the `selection` Snapshot field; the clamp edge guards bounds handling
- status: TODO

### L1.EDITOR.022 — Toggle word wrap is editor-scoped (per-editor, not config-backed)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile, setting]
- precondition: open a file; `setting {key:"editor.wordWrap"}` captured as W
- action: executeCommand `editor.action.toggleWordWrap`
- expected: word wrap toggles for the active editor view WITHOUT changing the persisted `editor.wordWrap` config value
- assert: `setting {key:"editor.wordWrap"}` value == W after toggle (config unchanged — toggle is a per-editor view override, not a settings write)
- why: documents the per-editor-view vs config-backed distinction (contrast L1.EDITOR.023 / settings area); a refactor writing config on toggle would change W and trip this
- status: partial(`settings.toggleMinimap` toggles a config-backed setting via `editor.action.toggleMinimap`; the per-editor word-wrap distinction is untested)

### L1.EDITOR.023 — Select All selects the whole document
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, selection]
- precondition: `PROJECT/fleet-selall.txt` written with 3 lines, opened, cursor at start
- action: executeCommand `editor.action.selectAll`
- expected: the selection spans from (0,0) to the document end
- assert: `after.selection.start == {line:0,character:0}` AND `after.selection.end.line == 2` (last line index)
- why: guards the `selection` Snapshot field reporting a real multi-line range; foundation for selection-based input tests (see 1a-input)
- status: implemented (editor.selectAll)

### L1.EDITOR.024 — Navigate Back / Forward restores prior editor positions
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: open `a.txt`, then open `b.txt` (now active); navigation history has a→b
- action: executeCommand `workbench.action.navigateBack` then `workbench.action.navigateForward`
- expected: back returns activeEditor to `a.txt`; forward returns it to `b.txt`
- assert: after back, `activeEditor` basename == `a.txt`; after forward, `activeEditor` basename == `b.txt`
- edges: navigateBack with empty history (fresh env, one file) → no-op, activeEditor unchanged
- why: guards the cross-editor navigation stack reflecting in activeEditor; empty-history edge guards bounds
- status: TODO

### L1.EDITOR.025 — Toggle fold/unfold collapses a foldable region (needs +lang)
- layer: L1
- scenarios: [python, node]
- isolation: fresh
- needs: [openFile, writeFile, selection]
- precondition: a file with a foldable block (e.g. a multi-line `def`/`function`); cursor inside it
- action: executeCommand `editor.fold` then `editor.unfold`
- expected: fold collapses the region (folding requires language/indentation model); unfold restores
- assert: best-effort visible-line proxy — command returns ok both times; if a `foldedRanges` snapshot field exists, it gains/loses one entry. Marked partial because the snapshot has no fold field today
- edges: fold on a non-foldable single-line file → no-op, no error
- why: guards the folding commands running on a +lang host; honest that fold state is not yet a Snapshot observable
- status: partial(no `foldedRanges` Snapshot field — only command-ok assertable until Track-D adds one)

### L1.EDITOR.026 — Concurrent edits to two split panes of the same doc stay consistent
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile, typeText, splitEditor, saveAll, fileContent]
- precondition: open `shared.txt`, `workbench.action.splitEditor` so the SAME doc shows in two visible groups
- action: focus group 1, `typeText "AAA"`; focus group 2 (`workbench.action.focusNextGroup`), `typeText "BBB"`; `saveAll`
- expected: both edits land in the one underlying document model (split panes share the model)
- assert: `fileContent {path}` `.text` contains BOTH `AAA` and `BBB`; `exec cat` confirms on disk
- edges: concurrent edge — same doc, two panes, ensures no split-brain buffer
- why: split shows one model in two groups; guards that edits in either pane mutate the shared model, not divergent copies
- status: TODO

### L1.EDITOR.027 — Open a binary/non-text file does not corrupt the snapshot
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile]
- precondition: `exec` writes a small binary blob to `PROJECT/blob.bin` (`head -c 256 /dev/urandom > blob.bin`)
- action: request `openFile {path:PROJECT/blob.bin}`
- expected: VS Code opens it in a binary/preview editor (not a text editor); `editorText` is absent/empty, not garbage
- assert: reply `ok` (or graceful `ok:false`); `after.activeEditor` references `blob.bin` OR is unchanged; `after.editorText` is undefined/empty (no binary dumped into the text field)
- edges: failure-mode edge — non-text input must not crash the bridge or pollute editorText
- why: guards the bridge degrading gracefully on non-text editors; protects `editorText` from binary contamination
- status: TODO

### L1.EDITOR.028 — Open the same file twice focuses the existing tab (no duplicate)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: `c.txt` written; `openFile` it once → one tab, `openTabs.length == T`
- action: request `openFile {path:PROJECT/c.txt}` a SECOND time
- expected: no new tab; the existing tab refocuses
- assert: `after.openTabs.length === before.openTabs.length` (no +1); `activeEditor` basename == `c.txt`
- edges: repeat edge of L1.EDITOR.001 — idempotent open
- why: guards openFile idempotence; a regression opening duplicate tabs would inflate openTabs and leak editors
- status: implemented (editor.openSameNoDup)

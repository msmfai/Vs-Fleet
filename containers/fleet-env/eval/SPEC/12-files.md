# 12 — Files (create / open / rename / delete / move / explorer / quick-open / save-as)

L1 in-env file-management surface. Every entry drives a real VS Code command id or a
bridge action (`writeFile`/`openFile`/`saveAll`/`closeEditor`) and asserts the effect
via the bridge `query` snapshot (`activeEditor`, `openTabs`, `visibleEditors`), a
`fileContent` query, or an out-of-band `env.exec` shell read of the container fs at
`/home/coder/project`. No "verify it works" — every entry names the observable.

Workspace root inside the container: `/home/coder/project` (§8). Bridge actions return
`{type:"result",reqId,ok,...}`; query payloads land on the result msg or under `.data`
(read both shapes, per `behaviours/files.mjs`).

---

### L1.FILES.001 — Create a file via writeFile then open it → it is the active editor
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [writeFile, openFile, fileContent]
- precondition: `/home/coder/project/fleet-create.txt` does NOT exist; bridge connected
- action: `request{type:"writeFile", path:"/home/coder/project/fleet-create.txt", content:"FLEET_CREATE_OK\nline two\n"}` then `request{type:"openFile", path}`
- expected: the named file becomes the active editor of the active group
- assert: snapshot.activeEditor resolves to `fleet-create.txt` (basename-tolerant via `isActive`)
- machine-state: fsChanges +1 (one new file in docker diff)
- why: foundational write→open loop; if openFile stops focusing the doc (opens background / wrong group / preview-only) activeEditor catches it.
- status: implemented (behaviour `file.create`)

### L1.FILES.002 — writeFile flushes real bytes to disk (not an unsaved buffer)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, fileContent]
- precondition: `/home/coder/project/fleet-create.txt` absent
- action: `writeFile` the file with marker `FLEET_CREATE_OK`, then `query{type:"fileContent", path}`
- expected: fileContent reads back bytes including the exact marker
- assert: `field(fc,"text").includes("FLEET_CREATE_OK")` (inclusion, not equality — tolerate BOM/trailing-newline normalisation)
- why: proves writeFile hits the workspace fs, not just a buffered model; the orthogonal half of file.create.
- status: implemented (behaviour `file.create`)

### L1.FILES.003 — Create file in a missing parent directory (edge: mkdir -p semantics)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, fileContent]
- precondition: `/home/coder/project/nope/` does NOT exist
- action: `writeFile{path:"/home/coder/project/nope/deep/a.txt", content:"X"}`
- expected: EITHER the parent dirs are created and the file lands, OR the bridge replies `ok:false` with an error — never a silent no-op that reports ok with no file
- assert: if `ok:true` then `env.exec("test -f /home/coder/project/nope/deep/a.txt && echo yes")=="yes"`; if `ok:false` then `error` is non-empty AND `env.exec("test -e /home/coder/project/nope")=="no"`
- edges: missing parent dir
- why: guards against writeFile silently reporting success while writing nothing (the worst regression — every downstream file test would false-pass).
- status: TODO

### L1.FILES.004 — Open a file that does not exist (edge: missing precondition)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [openFile]
- precondition: `/home/coder/project/ghost.txt` absent
- action: `request{type:"openFile", path:"/home/coder/project/ghost.txt"}`
- expected: the bridge does not crash; either it creates an empty editor for the path or replies `ok:false`. activeEditor is NOT silently left pointing at a stale prior editor.
- assert: bridge reply received within 3s (no hang); if `ok:true` snapshot.activeEditor basename == `ghost.txt`; if `ok:false` `error` non-empty
- edges: open missing file
- why: a missing-file open must be a defined outcome, not a hang or a silent focus of the wrong tab.
- status: TODO

### L1.FILES.005 — New untitled text file via command → an untitled editor becomes active
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: bridge connected; note current openTabs count
- action: `executeCommand "workbench.action.files.newUntitledFile"`
- expected: a new untitled editor opens and becomes active; openTabs grows by 1
- assert: snapshot.openTabs.length delta == +1 AND snapshot.activeEditor matches /Untitled|untitled/ (or is a non-disk scheme)
- edges: repeat — firing it twice yields two distinct untitled editors (openTabs +2, not deduped)
- why: untitled docs are the create-from-scratch path with no backing file; guards that the command registers a new in-memory editor and the snapshot tracks editors with no fs path.
- status: TODO

### L1.FILES.006 — Save an untitled file as a named file (Save As) → file appears on disk
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, typeText, saveAll]
- precondition: an untitled editor is active (via L1.FILES.005) with typed text `SAVEAS_OK`
- action: type into the untitled doc, then drive save; assert the named path on disk
- expected: bytes land at the chosen path on the container fs
- assert: `env.exec("cat /home/coder/project/fleet-saveas.txt").includes("SAVEAS_OK")` after save
- edges: Save As is interactive (`workbench.action.files.saveAs` opens a path picker the headless host can't drive) — so this MUST be modelled as type→writeFile-to-named-path→saveAll, or flagged that the real saveAs widget is undriveable headless
- why: the user-facing "save untitled as X" outcome is named bytes on disk; documents that the saveAs *widget* is not headless-driveable and we assert the end state instead.
- status: partial(saveAs widget undriveable headless; only end-state asserted via writeFile path)

### L1.FILES.007 — Close the active (Welcome) tab → open tab count shrinks
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command, query]
- precondition: fresh workbench with the Welcome/Get-Started tab open
- action: `executeCommand "workbench.action.closeActiveEditor"`
- expected: openTabs strictly decreases by one
- assert: snapshot.openTabs.length after < before (delta-based; exact fresh count is env-dependent); if openTabs absent → report "not measurable", pass=false
- edges: close with NO editor open → command is a no-op, openTabs stays 0 (not an error); covered by L1.FILES.008
- why: cheapest tripwire that the command channel round-trips a real workbench mutation AND the snapshot reflects editor-lifecycle changes; a break here invalidates the whole observe→act→observe loop.
- status: implemented (behaviour `file.openWelcomeClose`)

### L1.FILES.008 — Close active editor with no editors open (edge: empty state)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: close all editors first (repeat closeActiveEditor until openTabs==0)
- action: `executeCommand "workbench.action.closeActiveEditor"` once more
- expected: no-op; openTabs stays 0; no error reply
- assert: snapshot.openTabs.length == 0 before AND after; bridge reply `ok:true`
- edges: empty state
- why: closing nothing must be a benign no-op, not a thrown command error that would fail the act() transport.
- status: TODO

### L1.FILES.009 — Split editor right → a second editor becomes visible
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: `fleet-split.txt` seeded and opened (so there is a doc to split)
- action: `executeCommand "workbench.action.splitEditor"`
- expected: visibleEditors strictly grows (split clones the active doc into a new group beside it)
- assert: snapshot.visibleEditors.length after > before; if absent → pass=false "not measurable"
- edges: split with NO active editor → covered separately (L1.FILES.010)
- why: guards the tabs-vs-editor-groups distinction; if a snapshot refactor derives visibleEditors from openTabs (which split does NOT change) this fails.
- status: implemented (behaviour `editor.splitRight`)

### L1.FILES.010 — Split editor with no active editor (edge: nothing to split)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: all editors closed (openTabs==0, visibleEditors empty)
- action: `executeCommand "workbench.action.splitEditor"`
- expected: no-op or it opens an empty group; visibleEditors does NOT regress; no error
- assert: bridge reply `ok:true`; snapshot.visibleEditors.length after >= before
- edges: empty state
- why: split-with-nothing must not throw or corrupt the layout signal.
- status: TODO

### L1.FILES.011 — Type into an editor + saveAll → on-disk bytes reflect the edit
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [typeText, saveAll, writeFile, openFile]
- precondition: empty `fleet-save.txt` seeded and opened; `exec cat` shows it empty
- action: `typeText "FLEET_SAVED_MARKER"` then `request{type:"saveAll"}`
- expected: the marker reaches the backing file on disk
- assert: `env.exec("cat /home/coder/project/fleet-save.txt").includes("FLEET_SAVED_MARKER")` AND the before-read was empty
- machine-state: fsChanges includes the modified file
- why: the dirty→save→persist contract via an OUT-OF-BAND shell read — proves real durability, not that the editor merely believes it saved; isolates type-vs-save breaks via before/after evidence.
- status: implemented (behaviour `editor.saveDirty`)

### L1.FILES.012 — typeText lands in the focused editor, not a stale one (edge: focus routing)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [typeText, saveAll, writeFile, openFile]
- precondition: two files seeded+opened; `fleet-b.txt` opened last (so it is active)
- action: `typeText "ROUTED_HERE"` then `saveAll`
- expected: the text lands ONLY in the last-focused editor's file
- assert: `env.exec("cat fleet-b.txt").includes("ROUTED_HERE")` is true AND `env.exec("cat fleet-a.txt").includes("ROUTED_HERE")` is false
- edges: focus routing / wrong-editor regression
- why: a regression where typeText targets the first/wrong editor (instead of the active one) would silently corrupt the wrong file; this catches mis-routed keystrokes.
- status: TODO

### L1.FILES.013 — Rename a file on disk + reopen → tab references the new name
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile]
- precondition: `fleet-rename-old.txt` seeded + opened
- action: `env.exec("mv fleet-rename-old.txt fleet-rename-new.txt")`, then `closeActiveEditor`, then `openFile` the new path
- expected: the workbench surfaces the new basename and the new file exists on disk
- assert: snapshot openTabs/activeEditor references `fleet-rename-new.txt` (`refsPath`/`isActive`) AND `env.exec("test -f fleet-rename-new.txt")=="yes"`
- edges: a full `workbench.action.reloadWindow` would drop the bridge ws and hang — so rename is modelled as close-stale + open-new (documented constraint)
- why: protects the harness constraint (reload kills the bridge) AND guards openTabs reflecting basename changes (catching a label-cache regression).
- status: implemented (behaviour `file.rename`)

### L1.FILES.014 — Rename a file that is NOT open (edge: rename untracked-by-editor)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile]
- precondition: `fleet-orphan.txt` exists on disk but is NOT open in any editor
- action: `env.exec("mv fleet-orphan.txt fleet-orphan-renamed.txt")` then `openFile` the new path
- expected: old path gone on disk, new path present, new path opens as active
- assert: `env.exec("test -e fleet-orphan.txt")=="no"` AND `env.exec("test -f fleet-orphan-renamed.txt")=="yes"` AND snapshot.activeEditor basename == `fleet-orphan-renamed.txt`
- edges: rename a file with no editor model
- why: rename must work regardless of editor state; guards that openFile resolves a freshly-renamed path with no stale model interference.
- status: TODO

### L1.FILES.015 — Delete a file on disk while open → editor becomes stale; reopen fails cleanly
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile]
- precondition: `fleet-del.txt` seeded + opened (active editor)
- action: `env.exec("rm -f fleet-del.txt")`, then `request{type:"openFile", path}` again
- expected: file gone on disk; the re-open of the deleted path is a defined outcome (ok:false or an empty editor), not a hang
- assert: `env.exec("test -e fleet-del.txt")=="no"`; the second openFile returns a bridge reply within 3s (no hang)
- edges: delete-while-open / re-open deleted
- why: out-of-band delete is a common real event; the harness must not hang and the editor must surface a defined state.
- status: TODO

### L1.FILES.016 — Move a file into a subdirectory → new path opens, old path gone
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile]
- precondition: `fleet-move.txt` seeded; `/home/coder/project/sub/` created via `env.exec("mkdir -p sub")`
- action: `env.exec("mv fleet-move.txt sub/fleet-move.txt")` then `openFile "/home/coder/project/sub/fleet-move.txt"`
- expected: file lives under `sub/`, opens as active editor
- assert: `env.exec("test -f sub/fleet-move.txt")=="yes"` AND `env.exec("test -e fleet-move.txt")=="no"` AND snapshot.activeEditor path ends with `sub/fleet-move.txt`
- edges: move into a nested dir (path now contains a directory segment, not just basename)
- why: move = rename across dirs; guards that openFile resolves a multi-segment relative path and the snapshot reports the full path (not just basename collision with the old one).
- status: TODO

### L1.FILES.017 — Reveal the Explorer view → Explorer view container is active
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: bridge connected (any view may be focused)
- action: `executeCommand "workbench.view.explorer"`
- expected: the Explorer view container becomes the active/visible sidebar view
- assert: bridge reply `ok:true`; if snapshot exposes activeView/focusedView → it matches /explorer/i; else fall back to ok-returned (same posture as `search.findInFiles`)
- edges: repeat — calling it when already focused stays focused (idempotent, ok:true)
- why: Explorer is the file-tree entry point; guards command registration + viewlet activation. Dual posture because the snapshot may not expose the active view.
- status: TODO

### L1.FILES.018 — Quick-open a known file by name → it becomes the active editor
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [openFile]
- precondition: `fleet-quickopen.txt` seeded via `env.exec("printf ... > ...")` (so it works on the leanest bridge)
- action: `request{type:"openFile", path}` (the headless equivalent of Quick Open resolving a name)
- expected: the named file becomes the active editor
- assert: snapshot.activeEditor basename == `fleet-quickopen.txt` (`isActive`)
- edges: the Quick Open *widget* + fuzzy typing is a typeText concern; here we assert the navigation OUTCOME the widget produces
- why: name→file→focus navigation contract; a break isolated to this test (vs file.create) points at seed/resolution, not openFile focus mechanics.
- status: implemented (behaviour `quickOpen.byName`)

### L1.FILES.019 — Quick-open via the real widget (Go to File) + typeText (edge: widget path)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, typeText]
- precondition: `fleet-qo-widget.txt` seeded on disk
- action: `executeCommand "workbench.action.quickOpen"` then `typeText "fleet-qo-widget"` then `executeCommand "workbench.action.acceptSelectedQuickOpenItem"`
- expected: the file resolves and opens as active editor
- assert: snapshot.activeEditor basename == `fleet-qo-widget.txt` within 3s; if the quick-input overlay cannot be driven headless → flag as undriveable and SKIP
- edges: quick-input overlay may be undriveable headless (transient widget not exposed in snapshot)
- why: exercises the REAL Quick Open widget end-to-end (palette overlay + fuzzy match + accept), the part L1.FILES.018 deliberately skips; documents whether the overlay is headless-driveable.
- status: TODO

### L1.FILES.020 — Quick-open a name with no match (edge: no results)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command, typeText]
- precondition: no file named `zzz-does-not-exist` in the workspace
- action: `workbench.action.quickOpen` then `typeText "zzz-does-not-exist"`
- expected: no file opens; activeEditor unchanged; no error/crash
- assert: snapshot.activeEditor unchanged before vs after; bridge replies ok throughout (no hang)
- edges: empty/no-match state
- why: a no-match quick-open must not open a wrong/random file or hang the overlay.
- status: TODO

### L1.FILES.021 — saveAll with no dirty editors (edge: nothing to save)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [saveAll, query]
- precondition: no dirty editors (fresh workbench, no typed text)
- action: `request{type:"saveAll"}`
- expected: benign no-op; reply `ok:true` (`{saved:true}`); no file mtimes change
- assert: bridge reply `ok:true`; `env.exec` diff of an `ls --full-time` snapshot before/after shows no mtime change on workspace files
- edges: empty state (no dirty docs)
- why: saveAll must be safe to call when clean; a regression that touches/rewrites unchanged files would corrupt mtimes and trip incremental tooling.
- status: TODO

### L1.FILES.022 — Open the same file twice → it is not duplicated (edge: repeat open)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [openFile, writeFile, query]
- precondition: `fleet-dup.txt` seeded + already opened once
- action: `request{type:"openFile", path}` a second time
- expected: the existing editor is reused/focused, NOT a second tab for the same path
- assert: count of openTabs entries referencing `fleet-dup.txt` == 1 (not 2); activeEditor basename == `fleet-dup.txt`
- edges: repeat
- why: VS Code reuses an open document; a regression that opens a duplicate tab per call would leak editors and break tab-count assertions elsewhere.
- status: TODO

### L1.FILES.023 — Create + open a file with spaces and unicode in its name (edge: path escaping)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, fileContent]
- precondition: file `/home/coder/project/fleet space ünïcode.txt` absent
- action: `writeFile` then `openFile` that exact path with content `WEIRDNAME_OK`
- expected: the file is created and opens as active; fileContent reads back the marker
- assert: snapshot.activeEditor basename matches the unicode/space name AND `field(fileContent,"text").includes("WEIRDNAME_OK")` AND `env.exec` (with proper quoting) confirms the file exists
- edges: spaces + non-ASCII in path (escaping/quoting hazard across the bridge and exec)
- why: guards path-escaping bugs in the bridge wire and in `env.exec` quoting that only surface on non-trivial names.
- status: TODO

### L1.FILES.024 — Files written via bridge are visible to the container shell (same mount)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: `fleet-mount.txt` absent
- action: `writeFile{path, content:"MOUNT_OK"}`
- expected: the container shell sees the identical bytes at the same path
- assert: `env.exec("cat /home/coder/project/fleet-mount.txt") == "MOUNT_OK"` exactly
- edges: divergent-mount regression (editor fs != git/shell fs)
- why: every git + exec assertion in 13-scm-git.md depends on the editor's fs and the shell operating on ONE mount; this isolates a mount/path divergence as its own tripwire.
- status: TODO
</content>
</invoke>

# 13 — SCM / Git (init / stage / unstage / commit / branch / diff / decorations / discard / conflict)

L1 in-env git surface. Git plumbing assertions use the container shell via
`env.exec("cd /home/coder/project && git ...")` as the deterministic ground truth that
backs the SCM viewlet's decorations; the VS Code layer is driven via `workbench.view.scm`
and `git.*` command ids and cross-checked against the snapshot when it exposes an SCM
count. Tracked files are created through the `writeFile` bridge cap so the editor fs and
the git CLI agree on ONE workspace mount (see L1.FILES.024).

Identity is set per fresh repo: `git config user.email eval@fleet.local && git config
user.name "Fleet Eval"`. Workspace: `/home/coder/project`. Most entries are `fresh`
isolation because they mutate a repo. The snapshot field for SCM (when present) is
`snapshot.scmChanges` (opportunistic, `>=1` cross-checks, not authoritative).

> Note: the native menu (`mux.rs`) only forwards `workbench.view.scm`; the staging/commit
> command ids below (`git.stage`, `git.commit`, etc.) are the standard vscode.git
> extension ids, asserted against git plumbing because the SCM-UI path is non-deterministic
> headless. Several entries are TODO precisely because the headless git-UI driving needs
> the vscode.git extension active in the env image.

---

### L1.SCM.001 — git init → stage → commit yields exactly one commit
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [writeFile]
- precondition: empty project dir, no `.git`
- action: `env.exec("git init -q")` + identity config; `writeFile hello.txt`; `env.exec("git add -A && git commit -q -m 'fleet: initial commit'")`
- expected: one commit on HEAD, matching subject, clean working tree
- assert: `git rev-list --count HEAD == 1` AND `git log -1 --pretty=%s` includes `fleet: initial commit` AND `git status --porcelain == ""`
- why: foundational SCM lifecycle proven via git's own plumbing; also proves bridge-written files are git-visible (same mount). commits!=1 with dirty tree → suspect a mount/path mismatch between writeFile and PROJECT.
- status: implemented (behaviour `git.initStageCommit`)

### L1.SCM.002 — git init in a dir that is already a repo (edge: reinit)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: `git init` already run once (a `.git` exists, no commits)
- action: `env.exec("git init -q")` a second time
- expected: idempotent reinit; no error, no loss of any existing config
- assert: `env.exec` exit code 0 AND `git rev-parse --is-inside-work-tree == "true"` AND prior `user.email` config still reads `eval@fleet.local`
- edges: repeat / reinit
- why: reinit must be benign; guards a setup path that double-inits not corrupting identity config.
- status: TODO

### L1.SCM.003 — Open the Source Control view → SCM view container is active
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command]
- precondition: a git repo exists in the workspace
- action: `executeCommand "workbench.view.scm"`
- expected: the Source Control view becomes the active/visible sidebar view
- assert: bridge reply `ok:true`; if snapshot exposes activeView/focusedView → matches /scm|source.?control/i; else fall back to ok-returned
- edges: repeat — already-focused stays focused (idempotent)
- why: SCM viewlet entry point; guards `workbench.view.scm` registration + activation. Dual posture because snapshot may not expose the active view (mirrors search.findInFiles).
- status: TODO

### L1.SCM.004 — Open Source Control in a non-git folder (edge: no repo)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: project dir has NO `.git` (no `git init` run)
- action: `executeCommand "workbench.view.scm"`
- expected: the view opens and shows an empty/"no source control providers" state; no crash
- assert: bridge reply `ok:true`; snapshot.scmChanges (if present) == 0 or undefined; no error reply
- edges: empty/no-repo state
- why: SCM must degrade gracefully with no provider; a regression that throws on a non-repo workspace would break the viewlet for fresh projects.
- status: TODO

### L1.SCM.005 — Modify a tracked file → exactly one working-tree change surfaces
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [writeFile]
- precondition: committed baseline (init + write `tracked.txt` + commit); tree clean (`git status --porcelain == ""`)
- action: `writeFile tracked.txt` rewriting its first line; wait 800ms for the SCM provider to notice
- expected: git reports exactly one changed file
- assert: `git status --porcelain` has exactly 1 non-empty line AND clean-before was empty; if snapshot.scmChanges present cross-check `>= 1` (opportunistic, not `==1`)
- why: SCM gutter/badge decorations are a projection of git's working-tree diff; proves the bridge edit is git-visible. changed!=1 → edit missed the tracked path (mount mismatch) or extra files leaked.
- status: implemented (behaviour `git.diffDecorations`)

### L1.SCM.006 — A new untracked file shows as an untracked change
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: committed baseline, clean tree
- action: `writeFile untracked-new.txt` (a brand-new path), wait 800ms
- expected: git reports one untracked entry (`?? untracked-new.txt`)
- assert: `git status --porcelain` contains a line starting `??` for `untracked-new.txt` AND total changed lines == 1
- edges: untracked (vs modified) — different porcelain status code
- why: untracked and modified are distinct SCM states; guards that a new file is counted as a change at all (a regression could only watch tracked paths).
- status: TODO

### L1.SCM.007 — Stage a modified file via git.stage command → it moves to the index
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, command]
- precondition: committed baseline; `tracked.txt` modified (one working-tree change); SCM view open
- action: `openFile tracked.txt` then `executeCommand "git.stage"` (acts on the active resource)
- expected: the change moves from working-tree to the staged index
- assert: after staging, `git diff --cached --name-only` lists `tracked.txt` AND `git diff --name-only` (unstaged) no longer lists it
- edges: requires vscode.git extension active + a resolvable active SCM resource
- why: staging is the core SCM mutation; asserts via git's index plumbing (`--cached`) that the UI command actually wrote the index, not just repainted.
- status: TODO

### L1.SCM.008 — Unstage a staged file via git.unstage → it returns to working-tree
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, command]
- precondition: `tracked.txt` modified AND staged (`git add tracked.txt` done)
- action: `openFile tracked.txt` then `executeCommand "git.unstage"`
- expected: the change leaves the index and returns to the working tree
- assert: after, `git diff --cached --name-only` does NOT list `tracked.txt` AND `git diff --name-only` (unstaged) lists it again
- edges: the inverse of L1.SCM.007; round-trips the index
- why: unstage must exactly reverse stage; guards index-state correctness in both directions.
- status: TODO

### L1.SCM.009 — Stage all changes via git.stageAll → working tree empties into index
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command]
- precondition: committed baseline; two tracked files modified + one new untracked file
- action: `executeCommand "git.stageAll"`
- expected: all working-tree + untracked changes move to the staged index
- assert: `git diff --name-only` (unstaged) empty AND `git status --porcelain | grep '^??'` empty AND `git diff --cached --name-only` lists all three
- edges: mix of modified + untracked staged together
- why: stage-all is the bulk path; guards that untracked files are also added (not just modified), the common surprise in stage-all semantics.
- status: TODO

### L1.SCM.010 — Commit staged changes via git.commit with a message
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command, typeText]
- precondition: a baseline commit exists; `tracked.txt` modified + staged; SCM input box focused with message `fleet: second commit`
- action: type the message into the SCM input, `executeCommand "git.commit"`
- expected: HEAD advances by one commit with that subject; tree clean
- assert: `git rev-list --count HEAD == 2` AND `git log -1 --pretty=%s` includes `fleet: second commit` AND `git status --porcelain == ""`
- edges: the SCM input box is a specific widget; if it can't be driven headless, model via `git commit -m` and flag the UI gap
- why: commit is the durable SCM action; HEAD-count + subject prove the index was committed. Documents whether the SCM message input is headless-driveable.
- status: TODO

### L1.SCM.011 — Commit with an empty message (edge: no message)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command]
- precondition: staged change present; SCM message input empty
- action: `executeCommand "git.commit"` with no message
- expected: no commit is created; the editor prompts for / requires a message (HEAD does not advance)
- assert: `git rev-list --count HEAD` unchanged before vs after; `git diff --cached --name-only` still lists the staged file (still staged, uncommitted)
- edges: empty message
- why: an empty-message commit must NOT silently create a commit with a blank subject; guards the message-required precondition.
- status: TODO

### L1.SCM.012 — Commit with nothing staged (edge: empty index)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: clean tree, nothing staged, a baseline commit exists
- action: `executeCommand "git.commit"`
- expected: no commit created; no error/crash (the UI surfaces "nothing to commit")
- assert: `git rev-list --count HEAD` unchanged; bridge reply `ok:true` (command handled gracefully)
- edges: empty staging area
- why: committing nothing must be a benign no-op, not a thrown error or an empty commit.
- status: TODO

### L1.SCM.013 — Create a branch via git.branch → branch exists and is checked out
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command, typeText]
- precondition: a repo with at least one commit on the default branch
- action: `executeCommand "git.branch"` then type `feature/fleet-x` into the branch-name input
- expected: a new branch is created and HEAD switches to it
- assert: `git rev-parse --abbrev-ref HEAD == "feature/fleet-x"` AND `git branch --list feature/fleet-x` non-empty
- edges: the branch-name quick-input may be undriveable headless → model via `git checkout -b` and flag
- why: branch create+checkout is core; asserts via `rev-parse HEAD` that the command both created AND switched, not just created.
- status: TODO

### L1.SCM.014 — Create a branch with a name that already exists (edge: duplicate branch)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: branch `dup-branch` already exists
- action: attempt to create `dup-branch` again
- expected: creation is rejected; the existing branch and its tip are unchanged; no crash
- assert: `git branch --list dup-branch` still lists exactly one entry; its tip SHA unchanged before vs after; bridge reply does not leave the repo in a detached/half state (`git rev-parse --abbrev-ref HEAD` is a named branch)
- edges: duplicate name
- why: a duplicate-branch attempt must not clobber the existing branch or detach HEAD.
- status: TODO

### L1.SCM.015 — Switch branch via git.checkout → HEAD moves, working tree updates
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command]
- precondition: two branches `main` and `other`, with `other` containing a file `only-on-other.txt` not on `main`; currently on `main`
- action: `executeCommand "git.checkout"` and select `other`
- expected: HEAD moves to `other` and the working tree gains `only-on-other.txt`
- assert: `git rev-parse --abbrev-ref HEAD == "other"` AND `env.exec("test -f only-on-other.txt")=="yes"`
- edges: checkout that changes the working tree contents
- why: checkout must update both HEAD ref AND the working tree; asserting the file appears proves the tree was actually materialised, not just the ref pointer moved.
- status: TODO

### L1.SCM.016 — Discard a working-tree change via git.clean → file reverts to HEAD
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, command]
- precondition: committed `tracked.txt` with content `original\n`; then modified to `modified\n` (one working-tree change)
- action: `openFile tracked.txt` then `executeCommand "git.clean"` (discard changes) on the active resource
- expected: the file's on-disk content reverts to the committed `original\n`; tree clean
- assert: `env.exec("cat tracked.txt") == "original\n"` AND `git status --porcelain == ""`
- edges: discard is DESTRUCTIVE — must revert exactly to HEAD, not to empty
- why: discard restores HEAD content; an out-of-band `cat` proves the real bytes reverted. A regression that truncates instead of reverting would be caught here.
- status: TODO

### L1.SCM.017 — Discard an untracked file via git.cleanUntracked → file removed (edge: untracked)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command]
- precondition: committed baseline; one new untracked file `junk.txt`
- action: discard the untracked file via the SCM "discard" action (git clean for untracked)
- expected: the untracked file is deleted from disk; tracked files untouched
- assert: `env.exec("test -e junk.txt")=="no"` AND `git status --porcelain == ""`
- edges: discarding an untracked file DELETES it (vs reverting a tracked one)
- why: untracked-discard removes the file entirely — distinct from tracked-discard which reverts; guards the destructive-delete path and that it leaves tracked files alone.
- status: TODO

### L1.SCM.018 — Discard with no changes (edge: clean tree)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: committed baseline, clean tree
- action: invoke discard-all / `git.cleanAll`
- expected: no-op; tree stays clean; no error
- assert: `git status --porcelain == ""` before AND after; HEAD SHA unchanged; bridge reply `ok:true`
- edges: empty state
- why: discard on a clean tree must not delete or revert anything.
- status: TODO

### L1.SCM.019 — Diff a modified tracked file → diff shows the changed lines
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, command]
- precondition: committed `diff-me.txt` = `a\nb\nc\n`; then modified to `a\nB-CHANGED\nc\n`
- action: assert the diff content via git (the ground truth behind the SCM diff editor)
- expected: the diff shows exactly one changed line (b → B-CHANGED), lines a and c unchanged
- assert: `git diff --unified=0 diff-me.txt` output contains `-b` and `+B-CHANGED` AND does NOT contain `-a`/`-c`; if a snapshot diff view is exposed, cross-check it opened
- edges: opening the actual VS Code diff editor (`git.openChange`) is interactive; assert via git diff plumbing as the deterministic backing
- why: the SCM diff editor is a projection of `git diff`; asserting the hunk content proves only the changed line is flagged (guards a regression that whole-file-diffs an unchanged file).
- status: TODO

### L1.SCM.020 — SCM decoration count cross-checks git status (snapshot ↔ plumbing)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: committed baseline; modify exactly N=2 tracked files
- action: `writeFile` two tracked files with new content; wait 800ms for the SCM provider
- expected: git reports 2 changes AND (if exposed) snapshot.scmChanges agrees
- assert: `git status --porcelain` has exactly 2 non-empty lines; if `typeof snapshot.scmChanges === "number"` then `snapshot.scmChanges >= 2` (opportunistic; git is authoritative)
- edges: snapshot may not expose scmChanges (then git-only)
- why: validates the editor's SCM model stays in sync with git plumbing under a multi-file change; a snapshot/git mismatch flags an SCM-provider/watch regression, not a git one.
- status: partial(git side authoritative; snapshot.scmChanges cross-check only when exposed)

### L1.SCM.021 — Create + resolve a merge conflict → conflict markers then clean resolution
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: repo with `conflict.txt` committed on `main`; branch `feature` edits line 1 to `FEATURE`; `main` edits the same line 1 to `MAIN`; both committed on their branches
- action: `env.exec("git checkout main && git merge feature")` (expected to conflict), then resolve by `writeFile`-ing the resolved content + `git add` + `git commit`
- expected: the merge first produces conflict markers, then resolves to one merge commit; tree clean
- assert: mid-merge `git status` shows `UU conflict.txt` AND the file contains `<<<<<<<`/`=======`/`>>>>>>>`; after resolve `git status --porcelain == ""` AND `git rev-list --count HEAD` increased by the merge commit AND `env.exec("cat conflict.txt")` has neither marker nor the losing side
- edges: conflict state (`UU`), marker presence, then clean resolution
- why: conflict handling is the hardest SCM state; asserts the full lifecycle (conflict appears → markers present → resolved → clean) via git plumbing, the ground truth behind the merge-conflict UI.
- status: TODO

### L1.SCM.022 — Merge with no conflict (edge: clean fast-forward / auto-merge)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: `feature` adds a NEW file `feat.txt` (no overlap with `main`); on `main`
- action: `env.exec("git merge feature")`
- expected: merge succeeds with no conflict; `feat.txt` appears; tree clean
- assert: `git status --porcelain == ""` AND `env.exec("test -f feat.txt")=="yes"` AND no `<<<<<<<` anywhere in the tree
- edges: non-conflicting merge (the happy path that must NOT raise conflict markers)
- why: guards that a clean merge does not spuriously enter the conflict path; the negative case for L1.SCM.021.
- status: TODO

### L1.SCM.023 — SCM operations on a file outside the repo root (edge: nested/extra-repo path)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: repo at `/home/coder/project`; a file written at `/tmp/outside.txt` (outside the work tree)
- action: `writeFile /tmp/outside.txt`, then `git status --porcelain` in the project
- expected: the outside file does NOT appear as a git change (it is outside the work tree)
- assert: `git status --porcelain` in `/home/coder/project` does not reference `outside.txt`; total change count attributable to in-tree edits only
- edges: file outside the work tree must not pollute SCM state
- why: guards that the SCM/git boundary is the repo root; a regression that watches the whole fs would surface spurious changes.
- status: TODO

### L1.SCM.024 — Commit then verify decorations clear (post-commit clean state)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile]
- precondition: one tracked file modified (one SCM change, snapshot.scmChanges>=1 if exposed)
- action: `env.exec("git add -A && git commit -q -m 'fleet: clear'")`, wait 800ms for the SCM provider
- expected: the working-tree change count returns to zero; decorations clear
- assert: `git status --porcelain == ""` AND (if exposed) snapshot.scmChanges == 0 after; rev-count increased by 1
- edges: post-commit the change set must empty (decorations clear)
- why: completes the diff→commit→clean cycle; guards that the SCM provider re-reads after a commit and does not leave stale decorations.
- status: TODO

### L1.SCM.025 — git.refresh re-reads working tree after an out-of-band edit (edge: external change)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command]
- precondition: committed baseline, clean tree, SCM view open
- action: modify a tracked file with `env.exec` (bypassing the bridge, so no fs-watch event), then `executeCommand "git.refresh"`
- expected: the SCM model picks up the out-of-band change after refresh
- assert: `git status --porcelain` shows the 1 change (ground truth); if snapshot.scmChanges exposed, after refresh it is `>= 1`
- edges: out-of-band edit (no editor fs-watch event) requires explicit refresh
- why: exercises the manual-refresh path for changes the watcher may miss; documents that exec-edits need `git.refresh` to reflect in the UI snapshot.
- status: TODO
</content>

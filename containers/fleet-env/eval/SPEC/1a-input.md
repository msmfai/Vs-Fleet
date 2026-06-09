# L1.INPUT — Synthetic input: typeText / selection / keystrokes into editor + terminal

In-env (L1) coverage of Fleet's synthetic-input primitives: `typeText` (insert at the
active editor cursor), `termSend` (write a line to a terminal's pty stdin), and
cursor/selection-moving commands (`cursorBottom`, `editor.action.selectAll`, etc.)
that frame where typed text lands. Driven via the bridge; asserted on `fileContent`,
the `editorText` / `selection` Snapshot fields, `terminalText`, or out-of-band `exec`.

Background (from `behaviours/agentInput.mjs` + the bridge wire comment):
- `typeText {text}` → `{inserted:true}` — inserts at the **active text editor's**
  cursor. NOT a terminal, webview, or non-focused editor.
- `termSend {name?, text}` → `{terminal}` — `sendText(text + "\n")` to a named
  terminal, else the active one, else a freshly created one. The trailing newline
  means the line RUNS.
- `editorText` reflects the live document buffer; `selection` is
  `{start:{line,character}, end:{line,character}}` (caps advertised; Track-D/E Snapshot
  extensions).
- `terminalText {name?}` → `{text, source}` where `source ∈ {"buffer","captured",""}`;
  output capture relies on shell integration and is **racy** — prefer redirecting to a
  file and reading via `fileContent`/`exec` where determinism matters.

Workspace root is `/home/coder/project` (`PROJECT`).

---

### L1.INPUT.001 — typeText appends to the active editor's document
- layer: L1
- scenarios: [base, small-repo]
- needs: [writeFile, openFile, typeText, fileContent]
- precondition: `PROJECT/fleet-input.txt` written `"seed-line\n"` and opened (it is the active text editor); cursor moved to EOF via `cursorBottom`
- action: request `typeText {text:"FLEET_TYPED_OK"}` then (if supported) `saveAll`
- expected: the typed string appears appended after the seed line in the document
- assert: `fileContent {path}` `.text` includes `"FLEET_TYPED_OK"` (primary); `after.editorText` includes it (fallback). Seed line `seed-line` still present (append, not clobber)
- why: canary for the whole synthetic-input path — keys must land in the active text editor's model exactly as a human pressing keys would; `cursorBottom` anchors append vs overwrite
- status: implemented (behaviour `input.typeIntoEditor`)

### L1.INPUT.002 — typeText into a fresh untitled editor populates editorText
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [typeText]
- precondition: executeCommand `workbench.action.files.newUntitledFile` → an empty untitled editor is active
- action: request `typeText {text:"HELLO_UNTITLED"}`
- expected: the untitled buffer holds the typed text (no disk path to read, so assert via the snapshot)
- assert: `after.editorText` includes `"HELLO_UNTITLED"` (untitled has no file path → `editorText` is the only observable; `selection` cap required for editorText)
- edges: untitled (no-path) edge of L1.INPUT.001 — proves typeText works before any save target exists
- why: guards typeText landing in a path-less editor and surfacing via editorText, not just disk-backed files
- status: TODO

### L1.INPUT.003 — typeText with NO active editor is a clean no-op
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [closeEditor, typeText]
- precondition: all editors closed (`workbench.action.closeAllEditors`); `activeEditor == null`
- action: request `typeText {text:"NOWHERE"}`
- expected: no editor to receive input → reply `ok:false` OR `inserted:false`; nothing created on disk
- assert: reply not `inserted:true` (or `ok:false`); `after.activeEditor == null`; no new file in `PROJECT` (`exec ls`)
- edges: empty-state edge of L1.INPUT.001 — input with no target
- why: typeText must not silently create a phantom buffer or error-hang when there is no active editor
- status: TODO

### L1.INPUT.004 — typeText routes to the editor, NOT a focused terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, fileContent]
- precondition: open `PROJECT/route.txt`; then `workbench.action.terminal.new` (terminal now focused over the editor)
- action: request `typeText {text:"ROUTE_TO_EDITOR"}`
- expected: typeText targets the active TEXT EDITOR (`vscode.window.activeTextEditor`), not the terminal — so the text lands in `route.txt`'s buffer, NOT executed in the shell
- assert: `fileContent {path:PROJECT/route.txt}` `.text` includes `"ROUTE_TO_EDITOR"`; `terminalText` does NOT contain it (it was not typed into the pty)
- edges: failure-mode edge — guards the editor/terminal routing boundary
- why: typeText and termSend are distinct primitives; a regression routing typeText to a focused terminal would silently run keystrokes as shell commands
- status: TODO

### L1.INPUT.005 — Select-all then typeText replaces the entire document
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, selection, fileContent, saveAll]
- precondition: `PROJECT/replace.txt` written `"old line one\nold line two\n"`, opened; `editor.action.selectAll` (whole doc selected)
- action: request `typeText {text:"REPLACED"}` then `saveAll`
- expected: the selection is overwritten — the document becomes exactly `"REPLACED"`
- assert: `before.selection.end.line == 1` (selectAll spanned both lines); `fileContent` `.text` == `"REPLACED"` (no `old line` remains); `exec cat` confirms
- why: typeText replaces an active selection (VS Code insert-over-selection semantics); guards selection framing the insert — distinct from append (L1.INPUT.001)
- status: TODO

### L1.INPUT.006 — Cursor-position commands move where typeText inserts
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, selection, fileContent]
- precondition: `PROJECT/cursor.txt` written `"AAA\nBBB\n"`, opened; `cursorTop` (cursor at 0,0)
- action: request `typeText {text:"XX"}` (inserts at top), then `cursorBottom` + `typeText {text:"YY"}`
- expected: `XX` lands before `AAA`; `YY` lands after `BBB`
- assert: `fileContent` `.text` starts with `XX` and ends with `YY` (e.g. `"XXAAA\nBBB\nYY"`); `before.selection.start == {0,0}` then EOF after cursorBottom
- why: proves cursor commands relocate the insertion point read by typeText — the `selection` field is the observable tying cursor moves to typed output
- status: TODO

### L1.INPUT.007 — Multi-line typeText inserts newlines into the buffer
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, fileContent, saveAll]
- precondition: empty `PROJECT/multiline.txt` opened
- action: request `typeText {text:"line1\nline2\nline3"}` then `saveAll`
- expected: three distinct lines are inserted (the `\n`s become real line breaks in the model)
- assert: `fileContent` `.text` == `"line1\nline2\nline3"`; `exec wc -l` of the file reports the expected line count
- edges: embedded-newline edge — guards typeText not escaping/stripping `\n`
- why: typeText must honour newlines as line breaks (not literal `\n`), so agents can write multi-line code in one call
- status: TODO

### L1.INPUT.008 — typeText preserves Unicode / multibyte characters
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, fileContent, saveAll]
- precondition: empty `PROJECT/unicode.txt` opened
- action: request `typeText {text:"héllo — 你好 🚀"}` then `saveAll`
- expected: the exact Unicode bytes land in the document and on disk
- assert: `fileContent` `.text` includes `"你好"` and `"🚀"`; `exec cat unicode.txt | grep -q '🚀'` succeeds (UTF-8 round-trip)
- edges: encoding edge — guards typeText/saveAll not mangling multibyte/emoji
- why: synthetic input must be encoding-safe; a regression to ASCII-only or surrogate-splitting corrupts real source files
- status: TODO

### L1.INPUT.009 — termSend runs a command and the output round-trips via terminalText
- layer: L1
- scenarios: [base]
- needs: [termSend, terminalText]
- precondition: one terminal open (`workbench.action.terminal.new`)
- action: request `termSend {text:"echo FLEET_OK"}` (termSend appends `\n` → it RUNS)
- expected: the shell executes `echo FLEET_OK` and emits `FLEET_OK` to stdout, captured into the terminal buffer
- assert: poll `terminalText {name}` (15× / 800ms) until `.text` includes `"FLEET_OK"` (marker the prompt itself won't contain → proves execution, not just echo of the input line)
- why: first proof a terminal is a live interactive shell that runs commands and returns output; guards the termSend→pty→stdout→terminalText path
- status: implemented (behaviour `terminal.runEcho`)

### L1.INPUT.010 — termSend's cwd default is the workspace project root
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh terminal opened
- action: request `termSend {text:"pwd > /tmp/fleet-cwd.txt"}`
- expected: a new terminal's working directory is `/home/coder/project`
- assert: poll `fileContent {path:/tmp/fleet-cwd.txt}` (12× / 500ms) until `.text` includes `"/home/coder/project"` (file redirect read out-of-band, NOT racy terminalText output scraping)
- why: an agent opening a terminal expects the project root; guards the cwd default against a shell-profile `cd` or VS Code `terminal.integrated.cwd` override silently moving it
- status: implemented (behaviour `terminal.cwd`)

### L1.INPUT.011 — termSend to a named terminal targets that terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, terminalText]
- precondition: two terminals open; capture their names from `snapshot.terminals`
- action: request `termSend {name:<terminals[0]>, text:"echo NAMED_ONE"}`
- expected: only the named terminal receives the command; its buffer shows the marker
- assert: `terminalText {name:terminals[0]}` `.text` includes `"NAMED_ONE"`; `terminalText {name:terminals[1]}` `.text` does NOT
- edges: routing edge — guards termSend's name→terminal selection (vs always-active)
- why: agents drive specific terminals (build vs test panes); guards the named-target path of termSend
- status: TODO

### L1.INPUT.012 — termSend with NO terminal open creates one
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend]
- precondition: no terminals open; `snapshot.terminalCount == 0`
- action: request `termSend {text:"echo CREATED"}`
- expected: termSend creates a fresh terminal (the wire contract: "else a freshly created one") and runs the line
- assert: `after.terminalCount == 1`; reply `.terminal` is a non-empty name
- edges: empty-state edge of L1.INPUT.009 — send with nothing to send into
- why: termSend must self-provision a terminal when none exists, so input never silently drops
- status: TODO

### L1.INPUT.013 — termSend honours embedded newlines as multiple commands
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh terminal open
- action: request `termSend {text:"echo A > /tmp/seq.txt\necho B >> /tmp/seq.txt"}`
- expected: both lines execute in sequence (the embedded `\n` separates two commands; termSend's appended `\n` runs the last)
- assert: poll `fileContent {path:/tmp/seq.txt}` until `.text` contains both `A` and `B` on separate lines
- edges: multi-command edge of L1.INPUT.009
- why: guards termSend not collapsing/escaping embedded newlines so multi-step shell sequences run
- status: TODO

### L1.INPUT.014 — terminalText of an empty/unused terminal reports an empty source
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, terminalText]
- precondition: fresh terminal open, no command sent yet (only the shell prompt)
- action: request `terminalText {name:<the terminal>}`
- expected: reply `ok` with `source == "" ` (or `"captured"` with no command output) — no fabricated buffer
- assert: reply `ok:true`; `.text` does NOT contain a `$ ` command line we never sent; `.source` ∈ `{"","captured","buffer"}`
- edges: empty-state edge of L1.INPUT.009 — reading before any command ran
- why: guards terminalText honestly reporting empty/unpopulated buffers (it returns `source` precisely so callers know capture state) rather than inventing content
- status: TODO

### L1.INPUT.015 — termSend a long-running command does not block the bridge
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, terminalText]
- precondition: fresh terminal open
- action: request `termSend {text:"sleep 5 && echo SLEPT_DONE"}` then immediately request `query` (snapshot) and other commands
- expected: termSend returns promptly (it only writes to stdin, doesn't await completion); the bridge stays responsive during the sleep
- assert: the `termSend` reply arrives in <2s (well under the 5s sleep); a `query` issued right after also replies; later `terminalText` includes `"SLEPT_DONE"`
- edges: concurrency/non-blocking edge — guards termSend being fire-and-write, not await-completion
- why: agents launch long jobs; termSend must not wedge the observe/act channel waiting for the command to finish
- status: TODO

### L1.INPUT.016 — termSend a control sequence (Ctrl-C) interrupts a running command
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh terminal; start a blocking loop `termSend {text:"while true; do sleep 1; done"}`
- action: request `termSend {text:""}` (ETX / Ctrl-C) then `termSend {text:"echo INTERRUPTED > /tmp/int.txt"}`
- expected: Ctrl-C breaks the loop; the follow-up echo then runs (proving the shell returned to a prompt)
- assert: poll `fileContent {path:/tmp/int.txt}` until `.text` includes `"INTERRUPTED"` (only reachable if Ctrl-C freed the prompt)
- edges: control-character / failure-recovery edge — used by `agent.waitingState` to unblock a hung claude
- why: guards termSend delivering raw control bytes (not just printable text) so a stuck command can be interrupted — load-bearing for the agent-unblock path
- status: partial(`agent.waitingState` issues a Ctrl-C + pkill to unblock claude, but no behaviour asserts the interrupt itself frees the shell)

### L1.INPUT.017 — Concurrent typeText into two split panes of the same doc both land
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, saveAll, fileContent]
- precondition: open `PROJECT/concurrent.txt`, `workbench.action.splitEditor` (same model in two groups)
- action: `cursorBottom` + `typeText {text:"AAA"}` in group 1; `workbench.action.focusNextGroup` + `cursorBottom` + `typeText {text:"BBB"}` in group 2; `saveAll`
- expected: both typed strings land in the single shared document model
- assert: `fileContent` `.text` contains BOTH `AAA` and `BBB`; `exec cat` confirms
- edges: concurrent edge — two input sites, one model
- why: guards typeText respecting the shared model across split panes (no split-brain buffer); ties input to the L1.EDITOR.026 split-consistency invariant
- status: TODO

### L1.INPUT.018 — Repeated typeText accumulates (idempotence is NOT expected)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [writeFile, openFile, typeText, fileContent, saveAll]
- precondition: empty `PROJECT/accum.txt` opened, cursor at start
- action: request `typeText {text:"X"}` three times in a row, then `saveAll`
- expected: each call inserts at the (advancing) cursor → the buffer becomes `"XXX"`
- assert: `fileContent` `.text` == `"XXX"` (not `"X"`); proves each typeText advances the cursor and appends, not replaces
- edges: repeat edge of L1.INPUT.001 — confirms additive accumulation
- why: guards typeText being a genuine keystroke insert (cursor-advancing) rather than a set-buffer op; agents rely on incremental typing accumulating
- status: TODO

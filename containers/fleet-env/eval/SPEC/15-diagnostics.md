# 15 — Diagnostics (language diagnostics, Problems view, code actions / quick-fix)

L1 in-env area. Covers the diagnostics pipeline: language servers publishing markers,
the bridge `diagnostics` query / `snapshot.diagnostics` count, the Problems view
(`workbench.actions.view.problems`), Next/Previous-Problem navigation
(`editor.action.marker.next` / `editor.action.marker.prev`), and code-actions /
quick-fix (`editor.action.quickFix`, `editor.action.codeAction`,
`editor.action.organizeImports`, `editor.action.formatDocument`). Command ids are
verbatim from `crates/fleet-host/src/mux.rs` (GO/VIEW) and the standard VS Code action
registry.

**MOST DIAGNOSTICS NEED A +lang IMAGE.** The base `fleet-env:latest` ships no language
server, so on it `getDiagnostics()` is empty and `quickFix` finds nothing. Real
diagnostic content requires a Track-G variant image:
`fleet-env-python:latest` (Python LS), `fleet-env-node:latest` (JS/TS LS),
`fleet-env-rust:latest` (rust-analyzer). The `scenarios/repoLang.mjs` `langScenario`
helper degrades to `expectBoot:"fail"` (clean SKIP) when the variant image is absent,
so every +lang test below lists its `scenarios:` accordingly and the suite stays green
on a base-only box.

Observables:
- `snapshot.diagnostics` = `vscode.languages.getDiagnostics().reduce(...len)` — the
  total marker count across all files (a number).
- `{type:"diagnostics", detailed:true} → {items:[{file, sev, msg, line}]}` where
  `sev ∈ {error, warning, info, hint}` (from `DiagnosticSeverity` index). This is the
  per-marker assertion surface — name the exact `file`/`sev`/`msg`/`line` field.
- The Problems view focus is NOT in the snapshot, so opening it asserts dispatch only.

Reusable caps: `command, query, diagnostics, openFile, writeFile, saveAll,
fileContent, editorText`.

---

### L1.DIAG.001 — Problems view opens via the canonical command
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command, query]
- precondition: workbench booted, ext-host online
- action: executeCommand "workbench.actions.view.problems"
- expected: command resolves ok; snapshot.diagnostics is surfaced as evidence (the command reveals, it does not create/clear markers)
- assert: `env.act("workbench.actions.view.problems")` returns without throw; evidence records snapshot.diagnostics (number or "n/a")
- why: smoke test for the VIEW-menu Problems command id + that the snapshot's diagnostics field is alive. The focused-view name is not in the snapshot, so the honest observable is dispatch + a diagnostics-count probe; an unexpected "n/a" flags a snapshot regression independent of the command.
- status: implemented (behaviour `problems.open`)

### L1.DIAG.002 — Base image with no language server reports zero diagnostics
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [diagnostics, query]
- precondition: base image (no LS), no files seeded with errors
- action: `{type:"diagnostics", detailed:true}` query and read snapshot.diagnostics
- expected: `items.length == 0` and snapshot.diagnostics == 0
- assert: diagnostics query `items` array empty AND snapshot.diagnostics === 0
- why: EDGE (empty state) — establishes the base-image baseline so a later non-zero count on a +lang image is attributable to the language server, not ambient markers. Guards against phantom diagnostics on the bare image.
- status: TODO

### L1.DIAG.003 — Python language server flags an unused import + syntax error
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [diagnostics]
- precondition: +python scenario active; `setup` seeded `/home/coder/project/sample.py` = `import os\nx =\n`
- action: open sample.py, wait for the Python LS to publish markers, query `{type:"diagnostics", detailed:true}`
- expected: `items.length >= 1`; at least one item has `file` ending `sample.py` and `sev ∈ {error, warning}` for the incomplete `x =` and/or unused `import os`
- assert: diagnostics `items` filtered to basename `sample.py` is non-empty; at least one has `sev=="error"` (the `x =` parse error)
- machine-state: LS process present (procs +N after open); mem Δ bounded
- why: proves the Python LS actually activates and publishes markers through the bridge on this image. Asserts on the hard syntax error (`x =`) so it does not depend on optional linters. Marked +python; SKIPs cleanly when the image is absent (langScenario → expectBoot:"fail").
- status: partial(scenario `python` seeds the file + declares needs:["diagnostics"]; no behaviour yet opens the file and asserts the items array)

### L1.DIAG.004 — Node JS/TS service flags a hard parse error
- layer: L1
- scenarios: [node]
- isolation: fresh
- needs: [diagnostics]
- precondition: +node scenario active; `setup` seeded `/home/coder/project/sample.js` = `const x = ;\n`
- action: open sample.js, wait for the JS/TS service, query `{type:"diagnostics", detailed:true}`
- expected: `items.length >= 1` with a `sample.js` item `sev=="error"` (the `const x = ;` parse error)
- assert: diagnostics items filtered to basename `sample.js` contains ≥1 with `sev=="error"`
- why: isolates Node/JS-TS activation + diagnostics plumbing through the bridge. Asserts on an unambiguous parse error so any working JS language service reports it. Marked +node; clean SKIP when the image is absent.
- status: partial(scenario `node` seeds sample.js + needs:["diagnostics"]; no asserting behaviour yet)

### L1.DIAG.005 — rust-analyzer reports a type mismatch (semantic, not just syntax)
- layer: L1
- scenarios: [rust]
- isolation: fresh
- needs: [diagnostics]
- precondition: +rust scenario active; `setup` seeded `/home/coder/project/sample.rs` = `fn main() { let x: i32 = "s"; }\n`
- action: open sample.rs, wait for rust-analyzer to reach full analysis (longer init), query `{type:"diagnostics", detailed:true}`
- expected: `items.length >= 1` with a `sample.rs` item `sev=="error"` whose `msg` references a type mismatch (e.g. matches `/mismatched types|expected .*i32/i`)
- assert: diagnostics items filtered to basename `sample.rs` contains ≥1 `sev=="error"` with `msg` matching the type-mismatch regex
- why: rust-analyzer's value over a parser is type checking; a type error proves it initialised and is doing semantic analysis, not just parsing. The msg regex is load-bearing — a syntax-only pass could mask a half-initialised analyzer. Marked +rust; clean SKIP when absent. Allow a longer wait (analyzer is slow to init).
- status: partial(scenario `rust` seeds sample.rs + needs:["diagnostics"]; no asserting behaviour yet)

### L1.DIAG.006 — Fixing the source clears the diagnostic (marker lifecycle)
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [diagnostics, writeFile, saveAll]
- precondition: +python scenario; sample.py has ≥1 error marker (DIAG.003 state)
- action: writeFile sample.py = `import os\nx = 1\nprint(os.getcwd())\n` (valid, uses os), saveAll, wait for the LS to re-publish
- expected: diagnostics for `sample.py` drop to 0 (the syntax error and unused-import warning are gone)
- assert: diagnostics `items` filtered to basename `sample.py` is empty after the fix + re-publish wait
- why: proves the diagnostics pipeline is live, not a one-shot snapshot — markers must clear when the code is fixed. Guards the LS→bridge update path for marker removal. +python only; clean SKIP when absent.
- status: TODO

### L1.DIAG.007 — Next Problem navigates to the next marker
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [command, openFile, diagnostics, selection]
- precondition: +python scenario; a file with ≥2 markers on different lines open and active; cursor at line 0
- action: executeCommand "editor.action.marker.next"
- expected: snapshot.selection.start.line moves to the line of a marker (>0, i.e. cursor jumped to the next diagnostic)
- assert: snapshot.selection.start.line after > before; the line matches a `line` field from the diagnostics query items
- why: guards the GO-menu "Next Problem" id and that it actually navigates the cursor onto a marker line (cross-checked against the diagnostics items' `line`). Names the exact snapshot field (`selection.start.line`). +python; clean SKIP when absent.
- status: TODO

### L1.DIAG.008 — Next Problem with zero diagnostics is a clean no-op
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query, selection]
- precondition: base image (no markers); a plain file open and active, cursor at line 0
- action: executeCommand "editor.action.marker.next"
- expected: command resolves ok; snapshot.selection.start.line unchanged (no marker to jump to)
- assert: `env.act("editor.action.marker.next")` no throw; snapshot.selection after == before
- why: EDGE (empty state) — Next-Problem with no diagnostics must no-op, not throw or move the cursor arbitrarily; runs on base (no +lang needed) since the assertion is the absence of movement.
- status: TODO

### L1.DIAG.009 — Previous Problem navigates backward through markers
- layer: L1
- scenarios: [node]
- isolation: fresh
- needs: [command, openFile, diagnostics, selection]
- precondition: +node scenario; a file with ≥2 markers, cursor placed at/after the last marker
- action: executeCommand "editor.action.marker.prev"
- expected: snapshot.selection.start.line moves to an earlier marker line (< the starting line)
- assert: snapshot.selection.start.line after < before; matches a `line` from diagnostics items
- why: guards the GO-menu "Previous Problem" id symmetric to DIAG.007. +node; clean SKIP when absent.
- status: TODO

### L1.DIAG.010 — Quick Fix surfaces a code action on a fixable marker
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [command, openFile, diagnostics]
- precondition: +python scenario; sample.py has an unused-import marker (a fixable diagnostic); the marker's line is the active editor's cursor line
- action: executeCommand "editor.action.quickFix"
- expected: command resolves ok (opens the lightbulb/code-action menu at the cursor)
- assert: `env.act("editor.action.quickFix")` returns without throw
- why: guards the quick-fix command id at a position where a code action exists. The action menu is not snapshot-observable, so the honest observable is dispatch; DIAG.011 covers the applied effect. +python; clean SKIP when absent.
- status: TODO

### L1.DIAG.011 — Applying a quick-fix (organize/remove unused import) clears the marker + edits the file
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [command, openFile, diagnostics, fileContent]
- precondition: +python scenario; sample.py = `import os\nx = 1\n` (unused-import warning on line 0)
- action: executeCommand "editor.action.organizeImports" (or the remove-unused code action) then wait for re-publish
- expected: `fileContent(sample.py)` no longer contains `import os`; diagnostics for sample.py drop the unused-import marker
- assert: `fileContent({path})` after-text `!/import os/`; diagnostics items for basename sample.py no longer include the unused-import warning
- why: proves a code action produces a REAL source edit + marker clear, not just a menu. Names both observables (disk content via fileContent, marker via diagnostics). +python; clean SKIP when absent.
- status: TODO

### L1.DIAG.012 — Quick Fix on a marker-free position resolves with no actions
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: base image; a plain file open, cursor on a line with no diagnostic
- action: executeCommand "editor.action.quickFix"
- expected: command resolves ok (empty code-action menu / no lightbulb) — not an error
- assert: `env.act("editor.action.quickFix")` no throw; follow-up snapshot returns (env responsive)
- why: EDGE (no applicable action) — quick-fix with nothing to fix must no-op cleanly; runs on base since the assertion is "dispatch + no wedge", needing no language server.
- status: implemented (behaviour `diag.quickFixNoActions`)

### L1.DIAG.013 — Diagnostics query is empty before the language server finishes activating
- layer: L1
- scenarios: [rust]
- isolation: fresh
- needs: [diagnostics]
- precondition: +rust scenario; sample.rs just opened, rust-analyzer NOT yet finished init
- action: query `{type:"diagnostics", detailed:true}` immediately, then again after a bounded wait
- expected: the immediate query may return `items.length == 0` (analyzer not ready); the later query returns ≥1 — i.e. diagnostics are eventually-consistent, not instant
- assert: assert the LATER query has ≥1 sample.rs error; record the immediate count as evidence (no hard assertion on the race) so the test is deterministic
- why: EDGE (timing / eventual consistency) — encodes that diagnostics arrive asynchronously and the assertion must wait, preventing flaky "0 markers" failures on the slow rust-analyzer. Determinism-over-realism: assert only the settled state. +rust; clean SKIP when absent.
- status: TODO

### L1.DIAG.014 — Invalid JSON produces a built-in diagnostic without a +lang image
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [diagnostics, writeFile, openFile]
- precondition: base image; write `${PROJECT}/bad.json` = `{ "a": 1, }\n` (trailing comma / or `{ "a": }`) and open it as activeEditor
- action: wait for VS Code's built-in JSON language feature to publish, query `{type:"diagnostics", detailed:true}`
- expected: `items` contains ≥1 entry for basename `bad.json` with `sev ∈ {error, warning}`
- assert: diagnostics items filtered to basename bad.json non-empty
- why: VS Code ships JSON diagnostics built-in (no extra LS), so this is the ONE diagnostics behaviour that runs on the base image — a cheap end-to-end proof the diagnostics query + snapshot.diagnostics count work without waiting on a +lang image. Guards the bridge diagnostics surface independently of Track-G.
- status: implemented (behaviour `diag.jsonError`)

### L1.DIAG.015 — Fixing invalid JSON clears the built-in diagnostic (base image)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [diagnostics, writeFile, saveAll]
- precondition: bad.json with a JSON error marker (DIAG.014 state)
- action: writeFile bad.json = `{ "a": 1 }\n` (valid), saveAll, wait for re-publish
- expected: diagnostics for bad.json drop to 0
- assert: diagnostics items filtered to basename bad.json empty after the fix
- why: base-image marker-lifecycle proof (pairs with DIAG.006 for +python) — diagnostics clear on fix without any external LS; guards the built-in JSON validator's update path.
- status: implemented (behaviour `diag.jsonClears`)

### L1.DIAG.016 — snapshot.diagnostics count matches the diagnostics query item count
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [diagnostics, query, writeFile, openFile]
- precondition: base image; write + open a bad.json that yields a known number of JSON markers
- action: read snapshot.diagnostics (the reduced count) AND `{type:"diagnostics", detailed:true}` items length
- expected: `snapshot.diagnostics === items.length` (the count field and the detailed query agree)
- assert: numeric equality between snapshot.diagnostics and `items.length`
- why: guards the two diagnostics surfaces against drift — the snapshot count is a `reduce` over `getDiagnostics()` and the query iterates the same source; if they disagree, one path regressed. Names both exact code paths.
- status: implemented (behaviour `diag.countMatchesItems`)

### L1.DIAG.017 — Format Document on a syntactically valid file rewrites whitespace, not content
- layer: L1
- scenarios: [node]
- isolation: fresh
- needs: [command, openFile, writeFile, fileContent]
- precondition: +node scenario; write `${PROJECT}/fmt.js` = `const x={a:1,b:2}\n` (valid but unformatted), open as activeEditor
- action: executeCommand "editor.action.formatDocument" then saveAll
- expected: `fileContent(fmt.js)` changes (spacing inserted, e.g. `{ a: 1, b: 2 }`) but the semantic tokens (`const`, `x`, `a`, `1`, `b`, `2`) are all still present
- assert: `fileContent({path})` after != before AND after still contains `const`, `a`, `1`, `b`, `2`
- why: format is a diagnostics-adjacent code-action surface backed by the LS formatter. Asserting "changed AND tokens preserved" proves real formatting, not corruption. +node (needs a JS formatter); clean SKIP when absent.
- status: TODO

### L1.DIAG.018 — Format Document with no formatter installed resolves without mutating the file
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, openFile, writeFile, fileContent]
- precondition: base image (no language formatter for the chosen type); write `${PROJECT}/plain.xyz` = `a  b  c\n`, open
- action: executeCommand "editor.action.formatDocument"
- expected: command resolves ok; `fileContent` unchanged (no formatter → no edit) — not an error
- assert: `env.act("editor.action.formatDocument")` no throw; `fileContent({path})` after === before
- why: EDGE (missing capability) — format with no registered formatter must no-op, not throw or blank the file; guards the no-formatter path on the base image.
- status: implemented (behaviour `diag.formatNoFormatter`)

### L1.DIAG.019 — Concurrent diagnostics queries during LS activity return consistent reqId-matched replies
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [diagnostics, openFile]
- precondition: +python scenario; sample.py open, LS publishing
- action: fire two `{type:"diagnostics", detailed:true}` queries back-to-back (two reqIds in flight) while the LS is active
- expected: both replies arrive, each matched to its own reqId; neither hangs; item counts are non-decreasing/consistent (no cross-talk)
- assert: both `env.request` promises resolve with their distinct reqIds; both return an `items` array (no error, no swapped payloads)
- why: EDGE (concurrent) — the bridge correlates by reqId; guards against two in-flight diagnostics queries swapping payloads or one wedging the ext-host during active LS publishing. Names the reqId correlation mechanism. +python; clean SKIP when absent.
- status: TODO

### L1.DIAG.020 — Multi-file diagnostics report markers across more than one file
- layer: L1
- scenarios: [python]
- isolation: fresh
- needs: [diagnostics, writeFile, openFile]
- precondition: +python scenario; seed two erroring files `a.py` = `x =\n` and `b.py` = `import nonexistent_mod_xyz\n`, open both
- action: wait for the LS to publish for both, query `{type:"diagnostics", detailed:true}`
- expected: `items` contains entries for BOTH basenames `a.py` and `b.py` (distinct `file` fields)
- assert: set of basenames in diagnostics items ⊇ {a.py, b.py}
- why: proves diagnostics aggregate across files (not just the active editor) — the Problems view's whole-workspace model. Guards the snapshot's reduce-over-all-files and the query's iterate-all-files paths. +python; clean SKIP when absent.
- status: TODO

### L1.DIAG.021 — Problems view opens cleanly when there are zero problems
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: base image, no diagnostics (snapshot.diagnostics == 0)
- action: executeCommand "workbench.actions.view.problems"
- expected: command resolves ok; snapshot.diagnostics stays 0 (opening the empty Problems view creates no markers)
- assert: `env.act` no throw; snapshot.diagnostics === 0 before and after
- why: EDGE (empty state) — revealing the Problems view with nothing in it must be a clean no-op that does not fabricate markers; guards the view command on an empty workspace.
- status: implemented (behaviour `diag.problemsEmpty`)

---

Traceability: DIAG.001 ← `problems.open`. DIAG.003 / DIAG.004 / DIAG.005 are partial —
the `python`/`node`/`rust` scenarios in `scenarios/repoLang.mjs` seed the erroring
sample files and declare `needs:["diagnostics"]`, but no behaviour yet opens the file
and asserts the diagnostics `items` array (the asserting half is TODO). DIAG.014–016
(built-in JSON diagnostics) run on the base image and are the highest-priority TODOs
since they need no +lang image. All other entries TODO; +lang entries SKIP cleanly via
the langScenario `expectBoot:"fail"` degrade when the variant image is absent.

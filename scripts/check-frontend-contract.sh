#!/usr/bin/env bash
# Static frontend guards for the Tauri host — no Node (uses python3, which CI and
# macOS already have), so it never spawns node or trips macOS file-access prompts.
#
#   1. Webview-safety lint (shell): forbid window.prompt/alert/confirm (and bare
#      prompt()/alert()/confirm()) in ui/. These no-op or behave divergently in
#      macOS WKWebView — the class of bug that silently broke "rename a session".
#   2. IPC-contract (python3): every invoke("name", {keys}) in ui/*.js must map to
#      a registered #[tauri::command] with MATCHING argument keys (snake_case ↔
#      camelCase), and no registered command may be dead (no frontend caller and
#      not on the documented backend-only allow-list).
#
# Usage: scripts/check-frontend-contract.sh   (exit 1 on any violation)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
HOST="$ROOT/crates/fleet-host"
UI="$HOST/ui"
fail=0

echo "==> webview-safety lint (no window.prompt/alert/confirm in ui/)"
bad="$(grep -nE '(window\.(prompt|alert|confirm)[[:space:]]*\()|(^|[^A-Za-z0-9_.])(prompt|alert|confirm)[[:space:]]*\(' "$UI"/*.js 2>/dev/null \
  | grep -vE '^[^:]*:[0-9]+:[[:space:]]*//' \
  | grep -vE '(domPrompt|closePrompt|openFolderPrompt)' || true)"
if [ -n "$bad" ]; then
  echo "FAIL: forbidden webview input API in ui/ — use the in-DOM domPrompt() overlay:"
  echo "$bad"
  fail=1
else
  echo "  ok — none found"
fi

echo "==> IPC-contract (invoke ↔ #[tauri::command]: names, arg keys, no dead commands)"
HOST="$HOST" UI="$UI" python3 - <<'PY' || fail=1
import os, re, sys, glob

host = os.environ["HOST"]
ui = os.environ["UI"]

# Commands intentionally registered without a frontend caller (documented). Empty
# now — `spawn_server` was removed as dead; keep this list honest and minimal.
BACKEND_ONLY = set()

def snake_to_camel(s):
    head, *rest = s.split("_")
    return head + "".join(w[:1].upper() + w[1:] for w in rest)

main_rs = open(os.path.join(host, "src/main.rs")).read()

# 1. Registered commands from generate_handler![...] (last path segment).
m = re.search(r"generate_handler!\s*\[(.*?)\]", main_rs, re.S)
registered = []
if m:
    for tok in m.group(1).split(","):
        tok = tok.strip()
        if tok:
            registered.append(tok.split("::")[-1])
registered = set(registered)

# 2. Command fn signatures (data params = not State/AppHandle/Window). Scan all
#    Rust sources for `#[tauri::command]` then the following `pub fn name(params)`.
src = "\n".join(open(p).read() for p in glob.glob(os.path.join(host, "src/*.rs")))
cmd_params = {}
for fm in re.finditer(r"#\[tauri::command\][\s\S]*?pub fn\s+(\w+)\s*\(([\s\S]*?)\)\s*(->|\{)", src):
    name, params = fm.group(1), fm.group(2)
    keys = []
    # Split top-level commas (ignore commas inside <...> generics).
    depth, cur, parts = 0, "", []
    for ch in params:
        if ch in "<([": depth += 1
        elif ch in ">)]": depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur); cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    for p in parts:
        p = p.strip()
        if not p or ":" not in p:
            continue
        pname, ptype = p.split(":", 1)
        ptype = ptype.strip()
        if "State<" in ptype or "AppHandle" in ptype or "Window" in ptype:
            continue
        keys.append(snake_to_camel(pname.strip()))
    cmd_params[name] = set(keys)

# 3. invoke("name", { ... }) calls in ui/*.js → name + object keys.
errors = []
invoked = set()
inv_re = re.compile(r'invoke\(\s*"([a-zA-Z_]+)"\s*(?:,\s*\{([^}]*)\})?')
for jf in glob.glob(os.path.join(ui, "*.js")):
    text = open(jf).read()
    for cm in inv_re.finditer(text):
        name, obj = cm.group(1), cm.group(2) or ""
        invoked.add(name)
        if name not in registered:
            errors.append(f'{os.path.basename(jf)}: invoke("{name}") is not registered in generate_handler!')
            continue
        # JS object keys: first identifier of each comma-separated entry.
        js_keys = set()
        for entry in obj.split(","):
            entry = entry.strip()
            if not entry:
                continue
            key = entry.split(":", 1)[0].strip()
            if re.fullmatch(r"[A-Za-z_]\w*", key):
                js_keys.add(key)
        expected = cmd_params.get(name, set())
        if js_keys != expected:
            errors.append(
                f'{os.path.basename(jf)}: invoke("{name}", {{{obj.strip()}}}) keys {sorted(js_keys)} '
                f'!= command params {sorted(expected)}'
            )

# 4. Dead commands: registered but never invoked and not allow-listed.
dead = registered - invoked - BACKEND_ONLY
for d in sorted(dead):
    errors.append(f'command "{d}" is registered but has no frontend caller (dead — wire a caller, mark backend-only, or remove)')

if errors:
    print("FAIL: IPC-contract violations:")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print(f"  ok — {len(invoked)} invoked commands match names + arg keys; no dead commands")
PY

if [ "$fail" -ne 0 ]; then
  echo "frontend-contract: FAILED"
  exit 1
fi
echo "frontend-contract: PASSED"

#!/usr/bin/env bash
# Static frontend guards for the Tauri host — PURE SHELL, no Node, so it runs in
# CI and locally without spawning node or tripping macOS file-access prompts.
#
#   1. Webview-safety lint: forbid window.prompt/alert/confirm (and bare
#      prompt()/alert()/confirm() calls) in ui/. These no-op or behave
#      divergently in macOS WKWebView — exactly the class of bug that silently
#      broke "rename a session" (window.prompt returns null, no dialog). Use the
#      in-DOM domPrompt() overlay instead.
#   2. IPC-contract: every invoke("name", …) in ui/*.js must correspond to a
#      command registered in src/main.rs's tauri::generate_handler![…]. Catches
#      silent command-name drift between the frontend and the Rust backend.
#
# Usage: scripts/check-frontend-contract.sh   (exit 1 on any violation)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
HOST="$ROOT/crates/fleet-host"
UI="$HOST/ui"
fail=0

echo "==> webview-safety lint (no window.prompt/alert/confirm in ui/)"
# Drop full-line comments first, then match the forbidden call patterns:
#  - window.prompt( / window.alert( / window.confirm(
#  - bare prompt( / alert( / confirm( not part of an identifier (domPrompt(,
#    closePrompt(, openFolderPrompt( are fine — they are preceded by a letter).
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

echo "==> IPC-contract (every invoke() name is a registered #[tauri::command])"
registered="$(awk '/generate_handler!/{f=1;next} /\]/{f=0} f' "$HOST/src/main.rs" \
  | sed -E 's/[[:space:]]//g; s/,$//; s/.*:://' | grep -E '^[A-Za-z_]+$' | sort -u)"
invoked="$(grep -ohE 'invoke\("[a-zA-Z_]+"' "$UI"/*.js | sed -E 's/invoke\("//; s/"//' | sort -u)"
missing=""
for name in $invoked; do
  printf '%s\n' "$registered" | grep -qx "$name" || missing="$missing $name"
done
if [ -n "$missing" ]; then
  echo "FAIL: invoke() calls in ui/ with no matching command in generate_handler![]:$missing"
  fail=1
else
  echo "  ok — all $(printf '%s\n' "$invoked" | grep -c . ) invoked commands are registered"
fi

if [ "$fail" -ne 0 ]; then
  echo "frontend-contract: FAILED"
  exit 1
fi
echo "frontend-contract: PASSED"

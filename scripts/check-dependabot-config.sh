#!/usr/bin/env bash
set -euo pipefail

config="${1:-.github/dependabot.yml}"

if [ ! -f "$config" ]; then
  echo "FAIL: missing Dependabot config: $config"
  exit 1
fi

if ! rg -q '^version:[[:space:]]*2$' "$config"; then
  echo "FAIL: Dependabot config must use version: 2"
  exit 1
fi

require_update() {
  local ecosystem=$1
  local directory=$2

  if ! awk -v ecosystem="$ecosystem" -v directory="$directory" '
    function clean(value) {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      gsub(/^"|"$/, "", value)
      gsub(/^'\''|'\''$/, "", value)
      return value
    }

    function finish_entry() {
      if (in_entry && current_ecosystem == ecosystem && current_directory == directory && interval == "weekly") {
        found = 1
      }
    }

    /^[[:space:]]*-[[:space:]]*package-ecosystem:[[:space:]]*/ {
      finish_entry()
      in_entry = 1
      current_ecosystem = $0
      sub(/^[[:space:]]*-[[:space:]]*package-ecosystem:[[:space:]]*/, "", current_ecosystem)
      current_ecosystem = clean(current_ecosystem)
      current_directory = ""
      interval = ""
      next
    }

    in_entry && /^[[:space:]]*directory:[[:space:]]*/ {
      current_directory = $0
      sub(/^[[:space:]]*directory:[[:space:]]*/, "", current_directory)
      current_directory = clean(current_directory)
      next
    }

    in_entry && /^[[:space:]]*interval:[[:space:]]*/ {
      interval = $0
      sub(/^[[:space:]]*interval:[[:space:]]*/, "", interval)
      interval = clean(interval)
      next
    }

    END {
      finish_entry()
      exit found ? 0 : 1
    }
  ' "$config"; then
    echo "FAIL: Dependabot config must include weekly $ecosystem updates for $directory"
    exit 1
  fi
}

require_update "github-actions" "/"
require_update "cargo" "/"
require_update "cargo" "/crates/fleet-host"
require_update "npm" "/packages/fleet-bridge"
require_update "npm" "/packages/extension"

echo "Dependabot config check passed."

#!/usr/bin/env bash
set -euo pipefail

intent="${1:-docs/release/LICENSE_INTENT.md}"
dco="${2:-DCO.md}"
contributing="${3:-CONTRIBUTING.md}"
pr_template="${4:-.github/PULL_REQUEST_TEMPLATE.md}"

require_file() {
  local file=$1
  if [ ! -f "$file" ]; then
    echo "FAIL: missing file: $file"
    exit 1
  fi
}

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  if ! rg -qi "$pattern" "$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

require_file "$intent"
require_file "$dco"
require_file "$contributing"
require_file "$pr_template"

require_text "$intent" 'MIT OR Apache-2\.0' "permissive source-alpha license intent"
require_text "$intent" 'LICENSE-MIT' "MIT license file reference"
require_text "$intent" 'LICENSE-APACHE' "Apache license file reference"
require_text "$intent" 'SPDX' "manifest SPDX metadata note"
require_text "$intent" 'Developer Certificate of Origin|DCO' "DCO contribution posture"
require_text "$intent" 'does not assign' "DCO copyright limitation"
require_text "$intent" 'copyright' "DCO copyright term"
require_text "$intent" 'does not give|does not provide' "DCO limitation phrasing"
require_text "$intent" 'relicensing' "DCO relicensing limitation"
require_text "$intent" 'Contributor License Agreement|CLA' "CLA revisit warning"
require_text "$intent" 'monetization plan|business-model decision|paid model' \
  "copyleft monetization caveat"
require_text "$intent" 'internal use|run a local developer tool internally' \
  "internal-use copyleft limitation"
require_text "$intent" 'distribution|hosted-service|hosted service' \
  "copyleft trigger boundary"
require_text "$intent" 'released versions remain' "released-version irrevocability warning"
require_text "$intent" 'available' "released-version availability term"
require_text "$intent" 'AGPL-3\.0-only' "AGPL contingency naming"
require_text "$intent" 'future hosted control plane|hosted-reseller|hosted service' \
  "hosted-service contingency trigger"
require_text "$intent" 'library/API crates permissive|library crates permissive|reusable library' \
  "permissive reusable-library posture"

require_text "$dco" 'Signed-off-by: Your Name <your\.email@example\.com>' \
  "example Signed-off-by line"
require_text "$dco" 'git commit -s' "git commit sign-off instruction"
require_text "$dco" 'right to submit' "right-to-submit certification"
require_text "$dco" 'project license' "project-license certification"

require_text "$contributing" 'Developer Certificate of Origin|DCO' \
  "DCO contribution policy"
require_text "$contributing" 'Signed-off-by' "Signed-off-by contribution instructions"
require_text "$contributing" 'No Contributor License Agreement|no CLA' \
  "no-CLA alpha posture"
require_text "$pr_template" 'Developer Certificate of Origin|DCO' \
  "DCO PR certification"
require_text "$pr_template" 'Signed-off-by' "Signed-off-by PR checklist"

echo "License intent check passed."

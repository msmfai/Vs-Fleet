// failed-ids.mjs — print a comma-separated list of behaviour ids that FAILED or
// ERRORED (not skipped, not passed) in a §3.5 JSON report. Used by `make eval`
// retry-on-flake to re-run only the failures. Empty output ⇒ nothing to retry.
//
//   node scripts/failed-ids.mjs <report.json>

import { readFileSync } from "node:fs";

const path = process.argv[2];
if (!path) { process.stderr.write("usage: failed-ids.mjs <report.json>\n"); process.exit(2); }

let report;
try {
  report = JSON.parse(readFileSync(path, "utf8"));
} catch (e) {
  process.stderr.write(`failed-ids: cannot read ${path}: ${e.message}\n`);
  process.exit(2);
}

const ids = new Set();
for (const r of report.results || []) {
  if (r.skipped) continue;
  if (r.error || r.pass === false) {
    if (r.behaviour && r.behaviour !== "(boot)") ids.add(r.behaviour);
  }
}
process.stdout.write([...ids].join(","));

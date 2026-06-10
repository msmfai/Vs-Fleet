#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { reportBaseDir, writeScreenshotMetadata } from "../lib/reviewContext.mjs";

const jsonPath = resolve(process.argv[2] || "artifacts/eval.json");

try {
  const report = JSON.parse(readFileSync(jsonPath, "utf8"));
  const result = writeScreenshotMetadata(report, { baseDir: reportBaseDir(jsonPath) });
  if (result.missing) process.exitCode = 1;
} catch (err) {
  process.stderr.write(`[eval] failed to tag screenshot metadata: ${err?.message || err}\n`);
  process.exit(2);
}

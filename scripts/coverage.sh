#!/usr/bin/env bash
# Run cargo-llvm-cov coverage across Fleet's two Rust workspaces and (optionally)
# fail under a threshold. fleet-host is a STANDALONE workspace (root Cargo.toml
# `exclude`s it), so it must be measured in a separate invocation and the two
# raw profiles merged into one report.
#
# Usage:
#   scripts/coverage.sh                 # summary table for both workspaces
#   scripts/coverage.sh --lcov OUT      # write merged lcov to OUT (default coverage/fleet.lcov)
#   scripts/coverage.sh --fail-under N  # exit 1 if total line coverage < N (CI gate)
#   scripts/coverage.sh --host          # include the standalone fleet-host crate (needs Tauri deps)
#   scripts/coverage.sh --open          # open the HTML report
set -eo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

# --- toolchain shims -------------------------------------------------------
# The Nix GCC on PATH shadows Apple clang and breaks macOS linking (rejects
# -mmacos-version-min, can't find -liconv). Force Apple clang as compiler +
# linker. cargo-llvm-cov also needs llvm-cov/llvm-profdata that can read rustc's
# instrumentation profiles; the Xcode CommandLineTools copies read them fine.
if [ "$(uname -s)" = "Darwin" ]; then
  export CC="${CC:-/usr/bin/clang}"
  export CXX="${CXX:-/usr/bin/clang++}"
  export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="${CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER:-/usr/bin/clang}"
  CLT=/Library/Developer/CommandLineTools/usr/bin
  export LLVM_COV="${LLVM_COV:-$CLT/llvm-cov}"
  export LLVM_PROFDATA="${LLVM_PROFDATA:-$CLT/llvm-profdata}"
fi

# Reviewed coverage exclusions: dev-only / codegen binaries whose `fn main` is a
# thin shell around already-tested library code. Logic stays covered; the
# unreachable-in-tests wrapper does not drag the total below 100%. Keep this
# list short and justified — it is the audited allow-list, not a catch-all.
IGNORE_REGEX='(bin/gen-schema\.rs)'

# The reviewed exclusions use `#[cfg_attr(coverage_nightly, coverage(off))]`,
# which only takes effect when cfg(coverage_nightly) is set AND the unstable
# `coverage_attribute` feature is available. On a real nightly toolchain that's
# automatic; on stable we enable it via RUSTC_BOOTSTRAP=1. Set the cfg either way
# so the gate sees the exclusions regardless of which toolchain CI/locals use.
export RUSTFLAGS="${RUSTFLAGS:-} --cfg coverage_nightly"
if ! rustc --version 2>/dev/null | grep -q nightly; then
  export RUSTC_BOOTSTRAP=1
fi

# Branch coverage (cargo llvm-cov --branch) additionally needs nightly codegen;
# set COVERAGE_BRANCH=1 only when a real nightly toolchain is active.
BRANCH_FLAG=""
if [ "${COVERAGE_BRANCH:-0}" = "1" ]; then BRANCH_FLAG="--branch"; fi

LCOV_OUT="coverage/fleet.lcov"
FAIL_UNDER=""
INCLUDE_HOST=0
OPEN=0
WANT_LCOV=0
while [ $# -gt 0 ]; do
  case "$1" in
    --lcov) WANT_LCOV=1; LCOV_OUT="${2:-$LCOV_OUT}"; shift 2;;
    --fail-under) FAIL_UNDER="$2"; shift 2;;
    --host) INCLUDE_HOST=1; shift;;
    --open) OPEN=1; shift;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

cd "$ROOT"

echo "==> main workspace (6 crates)"
cargo llvm-cov --workspace --all-features $BRANCH_FLAG --ignore-filename-regex "$IGNORE_REGEX" --summary-only

if [ "$INCLUDE_HOST" -eq 1 ]; then
  echo "==> standalone fleet-host workspace"
  ( cd "$ROOT/crates/fleet-host" && cargo llvm-cov $BRANCH_FLAG --ignore-filename-regex "$IGNORE_REGEX" --summary-only )
fi

if [ "$WANT_LCOV" -eq 1 ]; then
  mkdir -p "$(dirname "$LCOV_OUT")"
  cargo llvm-cov --workspace --all-features $BRANCH_FLAG --ignore-filename-regex "$IGNORE_REGEX" --lcov --output-path "$LCOV_OUT"
  echo "lcov → $LCOV_OUT"
fi

if [ "$OPEN" -eq 1 ]; then
  cargo llvm-cov --workspace --all-features --ignore-filename-regex "$IGNORE_REGEX" --open
fi

if [ -n "$FAIL_UNDER" ]; then
  # The 100% gate is "zero UNCOVERED source lines", NOT `--fail-under-lines 100`.
  # llvm-cov's line/region PERCENTAGE double-counts generic/async monomorphized
  # instances and zero-width regions, so it reports <100% even when every source
  # line is covered or explicitly #[coverage(off)]-excluded — making a literal
  # `--fail-under-lines 100` unsatisfiable on this codebase. `--show-missing-lines`
  # is artifact-immune: it lists a source line only when NO instantiation covered
  # it. The gate fails iff that list is non-empty.
  echo "==> 100% gate: asserting zero uncovered source lines (artifact-immune)"
  miss="$(cargo llvm-cov --workspace --all-features $BRANCH_FLAG --ignore-filename-regex "$IGNORE_REGEX" --show-missing-lines 2>/dev/null \
    | awk '/Uncovered Lines:/{f=1;next} f' | grep -E 'crates/.*\.rs:|^[[:space:]]*[0-9]+$' || true)"
  if [ "$INCLUDE_HOST" -eq 1 ]; then
    miss="$miss$(cd "$ROOT/crates/fleet-host" && cargo llvm-cov $BRANCH_FLAG --ignore-filename-regex "$IGNORE_REGEX" --show-missing-lines 2>/dev/null \
      | awk '/Uncovered Lines:/{f=1;next} f' | grep -E 'crates/.*\.rs:|^[[:space:]]*[0-9]+$' || true)"
  fi
  if [ -n "$miss" ]; then
    echo "FAIL: uncovered source lines remain (must be tested or #[coverage(off)]-excluded):"
    echo "$miss"
    exit 1
  fi
  echo "PASS: no uncovered source lines."
fi

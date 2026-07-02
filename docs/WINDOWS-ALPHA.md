# Windows support status — alpha

VS Fleet targets **macOS and Linux** as first-class platforms. Windows builds are
produced (the release ships `x64`/`arm64` NSIS `.exe` installers and the app
compiles + boots), but Windows is **alpha**: the multiplexer runs, yet some
platform behaviors are unix-shaped and untested on Windows.

## What's alpha on Windows
- **Agent state** — the reporter/bridge transport uses a **unix domain socket**
  (`ws+unix://`, `FLEET_BRIDGE_SOCKET`) on macOS/Linux; on Windows the bridge falls
  back to the TCP `FLEET_BRIDGE_URL`. The filesystem-socket path (and its tests) do
  not apply on Windows.
- **Path / filesystem / process-spawn semantics** — editor/docker binary resolution,
  `PATH` handling, and mkdir-over-a-file error behavior are asserted against unix
  semantics; Windows differs and is not yet validated.

## Why some tests are `cfg`/`skipIf`-gated off Windows
To keep **Windows CI honest and green**, tests that assert unix-only behavior are
gated so they run on macOS + Linux but are skipped on Windows, rather than failing:
- Rust: `#[cfg(not(target_os = "windows"))]` on
  `fleet-e2e::dod_11_suite_runs_on_macos_and_linux` (a loopback-WS DoD suite whose
  own name scopes it to macOS+Linux) and three unix-semantics `fleet-host::spawn`
  tests (`docker_bin_honors_override_then_falls_back`,
  `editor_resolved_accepts_files_and_path_hits_only`,
  `write_spawned_server_settings_errors_when_parent_is_a_file`).
- Node: `describe.skipIf(process.platform === "win32")("unix socket transport", …)`
  in the fleet-bridge suite (the `ws+unix` filesystem-socket tests).

These gates run on **Linux**, so the 100%-coverage gate (which runs on Linux) is
unaffected — no Windows-only code paths are being hidden from coverage.

## To promote Windows out of alpha
Validate + fix the unix-shaped behaviors on Windows (path/`PATH`/spawn, and either
a Windows AF_UNIX transport or a first-class TCP path with the same guarantees),
then replace the gates with real cross-platform tests.

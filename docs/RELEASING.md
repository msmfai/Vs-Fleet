# Releasing VS Fleet

Release builds are automated by `.github/workflows/release.yml`.

## Cutting a release

```sh
git tag v0.1.0
git push github v0.1.0
```

The workflow then:

1. **Packages the bridge VSIX** once on Linux (`packages/fleet-bridge/package-vsix.sh`).
2. **Builds bundles on six lanes**: macOS x64 (`macos-15-intel`), macOS arm64
   (`macos-latest`), Linux x64 (`ubuntu-latest`), Linux arm64
   (`ubuntu-24.04-arm`), Windows x64 (`windows-latest`), Windows arm64
   (`windows-11-arm`). Each lane:
   - builds `fleet-reporter` and stages it as a Tauri sidecar
     (`crates/fleet-host/binaries/fleet-reporter-<triple>`);
   - stages the VSIX at `crates/fleet-host/resources/fleet-bridge.vsix`;
   - runs `tauri build --config tauri.release.conf.json` (the overlay adds
     `externalBin` + `resources` only at release time, so plain `cargo build`
     never requires staged files);
   - **smoke tests the built binary** (`scripts/release-smoke.mjs`): launch,
     bridge phone-home of one session, rail tab appears + selects, screenshot.
     A lane that cannot pass smoke does not ship.
3. **Publishes a draft GitHub release** with all artifacts, `SHA256SUMS.txt`,
   and `docs/release/NOTES.md` as the body. Review the draft, then publish.

`workflow_dispatch` runs the same pipeline without publishing — use it to
exercise the lanes before tagging.

## Artifacts

- macOS: `.dmg` + `Fleet-<platform>.app.tar.gz`
- Linux: `.AppImage`, `.deb`, `.rpm`
- Windows: NSIS `-setup.exe`

All artifact names carry an OS/arch suffix; `SHA256SUMS.txt` covers every file.

## Icons

`crates/fleet-host/build.rs` derives all icon assets (`32x32.png`,
`128x128.png`, `icon.ico`, `Fleet.icns`) from the single source
`crates/fleet-host/icons/icon.png` in pure Rust on every platform — replace
that one file to rebrand. `scripts/refresh-icons.sh` remains as a local
macOS helper only.

## Signing / notarization (deferred)

Artifacts are intentionally **unsigned** while the alpha proves useful.
When release quality requires it, add as a separate phase:

- macOS: Developer ID certificate + notarization (`APPLE_CERTIFICATE`,
  `APPLE_ID` secrets; Tauri supports both natively via env vars).
- Windows: an Authenticode certificate (`tauri.conf.json > bundle > windows >
  certificateThumbprint` or signtool in the workflow).
- Linux: optionally sign packages / publish a signed apt/rpm repo.

Until then, the release notes carry the unsigned warning and checksum
verification instructions.

## Platform caveats

- `fleet-reporter --serve` and the `claude` shim are unix-only; Windows
  builds degrade to "no agent state" (see `crates/fleet-host/src/spawn.rs`
  and `crates/fleet-reporter/src/main.rs`).
- The nested `crates/fleet-host` workspace is covered by the `host` CI job
  on all three OSes, including the `no_keyboard_capture` regression test.

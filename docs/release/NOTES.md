# VS Fleet — alpha release

Cross-platform alpha builds of VS Fleet, the stateless multiplexer host for
VS Code web sessions.

## ⚠️ Unsigned builds

These artifacts are **not code-signed or notarized**. Your OS will warn you:

- **macOS**: right-click the app → Open (or `xattr -dr com.apple.quarantine Fleet.app`
  after verifying the checksum). Gatekeeper will refuse a plain double-click.
- **Windows**: SmartScreen will warn on the installer — "More info" → "Run anyway".
- **Linux**: no signature checks; verify the checksum below.

Verify downloads against `SHA256SUMS.txt`:

```sh
shasum -a 256 -c SHA256SUMS.txt --ignore-missing
```

## Install

| OS | Artifact | Notes |
| --- | --- | --- |
| macOS (Apple Silicon) | `*-macos-arm64.dmg` | drag Fleet.app to Applications |
| macOS (Intel) | `*-macos-x64.dmg` | drag Fleet.app to Applications |
| Linux x64 | `*-linux-x64.AppImage` / `.deb` / `.rpm` | AppImage: `chmod +x` and run |
| Linux arm64 | `*-linux-arm64.AppImage` / `.deb` / `.rpm` | needs WebKitGTK 4.1 at runtime |
| Windows x64 | `*-windows-x64-setup.exe` | NSIS installer |
| Windows arm64 | `*-windows-arm64-setup.exe` | NSIS installer |
| any | `fleet-bridge.vsix` | the bridge extension on its own — see below |

## VS Fleet and VS Code are separate installs

VS Fleet does **not** bundle, download, or modify VS Code. For local
sessions you install both, independently:

1. **VS Code** — from Microsoft, with the `code` CLI available. The first
   local session may make VS Code's own CLI download its `serve-web` server
   bundle into `~/.vscode/cli/`.
2. **VS Fleet** — the artifact for your OS above.

Fleet drives your VS Code install but stays out of it: sessions run with
Fleet-private data dirs, and the bridge extension is installed only into
those — never into your VS Code settings, profiles, or extensions.

Without VS Code, Fleet still launches and externally started sessions can
phone home on their own (the planned SSH/container session paths do not
need a local VS Code); only spawning *local* sessions requires it.

`fleet-bridge.vsix` is embedded in the app for local sessions — no manual
step needed. It is published separately for remote/container environments
that bake the bridge into their own code-server installs.

## Known alpha limitations

- **Unsigned** on every platform (signing/notarization is a later phase).
- **Windows**: agent-state reporting (the `fleet-reporter --serve` hook
  receiver and the `claude` terminal shim) is not yet supported — sessions
  and tabs work, but rail tabs do not light up with agent working/waiting
  state. Unix-socket fast paths are unavailable.
- **Linux**: behavior varies with the distro's WebKitGTK; tested on
  Ubuntu 24.04 (x64 and arm64) runners only.
- Every artifact passed a launch smoke test on its target OS/arch (launch,
  bridge phone-home registration, rail tab select, screenshot) — deeper
  end-to-end coverage is still macOS-centric.

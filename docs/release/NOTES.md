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

Fleet needs a local **VS Code** install (the `code` CLI) to spawn local
web sessions; externally started sessions phone home on their own.

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

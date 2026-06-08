/**
 * Fleet VS Code Extension — stub (S0 scaffold).
 *
 * Real implementation (env injection, PATH shim, reporter connection) lands in
 * EXTSKEL (S8), ENVINJ (S9), and SHIM (S10).
 *
 * This file is Open-VSX-publishable; no proposed APIs are used.
 * Engine constraint: ^1.93.0
 */

// The activate/deactivate exports are the VS Code extension contract.
// We import the vscode module type-only to satisfy TypeScript without
// requiring a real VS Code runtime at build time.

export function activate(): void {
    // S0 scaffold: no-op activation.
    // Real activation (connect to reporter, inject env, shim PATH) in S8–S10.
    console.log("fleet-extension: scaffold stub activated");
}

export function deactivate(): void {
    // No-op teardown for the scaffold.
}

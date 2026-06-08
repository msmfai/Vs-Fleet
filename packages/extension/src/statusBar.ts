/**
 * Fleet status bar item (EXTSKEL S8).
 *
 * Shows a compact indicator of the Fleet Hub connection status in the VS Code
 * status bar. No proposed APIs; only the stable `StatusBarItem` API (^1.93).
 *
 * Visual states:
 *   $(sync~spin) Fleet…          connecting
 *   $(circle-filled) Fleet       connected  (green-ish via color)
 *   $(circle-outline) Fleet      disconnected
 *   $(warning) Fleet             error
 */

import * as vscode from "vscode";
import { ConnectionStatus, ConnectionStatusEvent } from "./connection";

/** How the status bar item looks for each ConnectionStatus. */
interface StatusAppearance {
    text: string;
    tooltip: string;
    /** VS Code theme color id or undefined for the default foreground. */
    color?: string;
}

function appearanceFor(status: ConnectionStatus, detail: string): StatusAppearance {
    switch (status) {
        case "connecting":
            return {
                text: "$(sync~spin) Fleet",
                tooltip: `Fleet: connecting to Hub (${detail})`,
            };
        case "connected":
            return {
                text: "$(circle-filled) Fleet",
                tooltip: `Fleet: connected (${detail})`,
            };
        case "disconnected":
            return {
                text: "$(circle-outline) Fleet",
                tooltip: `Fleet: disconnected (${detail})`,
            };
        case "error":
            return {
                text: "$(warning) Fleet",
                tooltip: `Fleet: error — ${detail}`,
            };
    }
}

/**
 * Manages the Fleet status bar item lifetime.
 *
 * Owns a single `vscode.StatusBarItem` and updates it in response to
 * `ConnectionStatusEvent` notifications from `HubConnection`.
 */
export class FleetStatusBar {
    private _item: vscode.StatusBarItem;

    constructor(context: vscode.ExtensionContext) {
        // Priority 100 → appears near the left end of the right-aligned cluster.
        this._item = vscode.window.createStatusBarItem(
            vscode.StatusBarAlignment.Right,
            100
        );
        this._item.command = "fleet.showStatus";
        this._item.text = "$(sync~spin) Fleet";
        this._item.tooltip = "Fleet: initializing…";
        this._item.show();

        // Registered in subscriptions so VS Code disposes it on deactivation.
        context.subscriptions.push(this._item);
    }

    /** Update the status bar to reflect a new connection state. */
    update(ev: ConnectionStatusEvent): void {
        const a = appearanceFor(ev.status, ev.detail);
        this._item.text = a.text;
        this._item.tooltip = a.tooltip;
    }

    /** Access the underlying item (for tests). */
    get item(): vscode.StatusBarItem {
        return this._item;
    }
}

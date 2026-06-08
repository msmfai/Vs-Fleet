/**
 * Unit tests for FleetStatusBar (S8 EXTSKEL).
 *
 * The vscode module is replaced by the mock at src/__mocks__/vscode.ts.
 */

import * as vscode from "vscode";
import { makeExtensionContext, resetAllMocks } from "../__mocks__/vscode";
import { FleetStatusBar } from "../statusBar";

beforeEach(() => resetAllMocks());

// ── Construction ──────────────────────────────────────────────────────────────

describe("FleetStatusBar construction", () => {
    it("creates a status bar item on the Right alignment", () => {
        const ctx = makeExtensionContext();
        new FleetStatusBar(ctx as unknown as vscode.ExtensionContext);

        expect(vscode.window.createStatusBarItem).toHaveBeenCalledWith(
            vscode.StatusBarAlignment.Right,
            100
        );
    });

    it("shows the item immediately", () => {
        const ctx = makeExtensionContext();
        const bar = new FleetStatusBar(ctx as unknown as vscode.ExtensionContext);
        expect((bar.item.show as jest.Mock)).toHaveBeenCalled();
    });

    it("assigns fleet.showStatus as the command", () => {
        const ctx = makeExtensionContext();
        const bar = new FleetStatusBar(ctx as unknown as vscode.ExtensionContext);
        expect(bar.item.command).toBe("fleet.showStatus");
    });

    it("registers the item in context.subscriptions for cleanup", () => {
        const ctx = makeExtensionContext();
        new FleetStatusBar(ctx as unknown as vscode.ExtensionContext);
        expect(ctx.subscriptions.length).toBe(1);
        expect(typeof ctx.subscriptions[0].dispose).toBe("function");
    });
});

// ── update() ─────────────────────────────────────────────────────────────────

describe("FleetStatusBar.update()", () => {
    function makeBar() {
        const ctx = makeExtensionContext();
        const capturedItem = {
            text: "initial",
            tooltip: "initial",
            command: undefined as string | undefined,
            show: jest.fn(),
            hide: jest.fn(),
            dispose: jest.fn(),
        };
        (vscode.window.createStatusBarItem as jest.Mock).mockReturnValueOnce(capturedItem);
        const bar = new FleetStatusBar(ctx as unknown as vscode.ExtensionContext);
        return { bar, item: capturedItem };
    }

    it("shows a spinner icon while connecting", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "connecting", detail: "ws://…" });
        expect(item.text).toContain("sync~spin");
    });

    it("shows a filled-circle icon when connected", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "connected", detail: "ws://127.0.0.1:51777" });
        expect(item.text).toContain("circle-filled");
    });

    it("shows an outline-circle icon when disconnected", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "disconnected", detail: "closed" });
        expect(item.text).toContain("circle-outline");
    });

    it("shows a warning icon on error", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "error", detail: "ECONNREFUSED" });
        expect(item.text).toContain("warning");
    });

    it("includes Fleet in every state's text", () => {
        const { bar, item } = makeBar();
        for (const status of ["connecting", "connected", "disconnected", "error"] as const) {
            bar.update({ status, detail: "x" });
            expect(item.text).toContain("Fleet");
        }
    });

    it("tooltip carries the detail for error status", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "error", detail: "ECONNREFUSED 127.0.0.1:51777" });
        expect(item.tooltip).toContain("ECONNREFUSED");
    });

    it("tooltip carries the endpoint for connected status", () => {
        const { bar, item } = makeBar();
        bar.update({ status: "connected", detail: "ws://127.0.0.1:51777" });
        expect(item.tooltip).toContain("51777");
    });
});

/**
 * Unit tests for the reporter supervisor (REPSERVE) — spawning/supervising the
 * per-window `fleet-reporter --serve` child. The spawner is injected so no real
 * process is ever started.
 */

import { EventEmitter } from "events";

import { ReporterSupervisor, reporterArgs } from "../reporter";

/** A fake child process: an EventEmitter with a kill() spy. */
function makeFakeChild(): EventEmitter & { kill: jest.Mock } {
    const ee = new EventEmitter() as EventEmitter & { kill: jest.Mock };
    ee.kill = jest.fn();
    return ee;
}

const BASE = {
    binPath: "/usr/local/bin/fleet-reporter",
    reporterSocket: "/run/fleet/reporter-win.sock",
    sessionId: "win-1",
};

describe("reporterArgs()", () => {
    it("builds --serve --session-id --socket plus --ws for a ws endpoint", () => {
        expect(
            reporterArgs({ ...BASE, hubEndpoint: "ws://127.0.0.1:51777" })
        ).toEqual([
            "--serve",
            "--session-id",
            "win-1",
            "--socket",
            "/run/fleet/reporter-win.sock",
            "--ws",
            "ws://127.0.0.1:51777",
        ]);
    });

    it("uses --unix for a unix: endpoint (Hub fast path)", () => {
        const args = reporterArgs({ ...BASE, hubEndpoint: "unix:/run/fleet/hub.sock" });
        expect(args).toContain("--unix");
        expect(args).toContain("/run/fleet/hub.sock");
        expect(args).not.toContain("--ws");
    });
});

describe("ReporterSupervisor", () => {
    it("spawns the reporter with the right binary, args, and FLEET_REPORTER_SOCKET env", () => {
        const child = makeFakeChild();
        const spawnFn = jest.fn(() => child) as unknown as typeof import("child_process").spawn;
        const sup = new ReporterSupervisor({
            ...BASE,
            hubEndpoint: "ws://127.0.0.1:51777",
            spawnFn,
        });
        sup.start();

        expect(spawnFn).toHaveBeenCalledTimes(1);
        const [bin, args, opts] = (spawnFn as jest.Mock).mock.calls[0];
        expect(bin).toBe(BASE.binPath);
        expect(args).toEqual(reporterArgs({ ...BASE, hubEndpoint: "ws://127.0.0.1:51777" }));
        expect(opts.env.FLEET_REPORTER_SOCKET).toBe(BASE.reporterSocket);
        expect(sup.startCount).toBe(1);
    });

    it("is idempotent while a child is alive (no double-spawn)", () => {
        const child = makeFakeChild();
        const spawnFn = jest.fn(() => child) as unknown as typeof import("child_process").spawn;
        const sup = new ReporterSupervisor({ ...BASE, hubEndpoint: "ws://h", spawnFn });
        sup.start();
        sup.start();
        expect(spawnFn).toHaveBeenCalledTimes(1);
    });

    it("restarts the reporter after an unexpected exit (with backoff)", () => {
        jest.useFakeTimers();
        const children = [makeFakeChild(), makeFakeChild()];
        let i = 0;
        const spawnFn = jest.fn(() => children[i++]) as unknown as typeof import("child_process").spawn;
        const sup = new ReporterSupervisor({
            ...BASE,
            hubEndpoint: "ws://h",
            restartDelayMs: 500,
            spawnFn,
        });
        sup.start();
        expect(sup.startCount).toBe(1);

        // The child exits unexpectedly → a restart is scheduled after the delay.
        children[0].emit("exit", 1, null);
        expect(sup.startCount).toBe(1); // not yet
        jest.advanceTimersByTime(500);
        expect(sup.startCount).toBe(2);
        expect(spawnFn).toHaveBeenCalledTimes(2);
        jest.useRealTimers();
    });

    it("dispose() kills the child and cancels a pending restart", () => {
        jest.useFakeTimers();
        const child = makeFakeChild();
        const spawnFn = jest.fn(() => child) as unknown as typeof import("child_process").spawn;
        const sup = new ReporterSupervisor({ ...BASE, hubEndpoint: "ws://h", restartDelayMs: 500, spawnFn });
        sup.start();
        sup.dispose();
        expect(child.kill).toHaveBeenCalled();

        // An exit after dispose must NOT schedule a restart.
        child.emit("exit", 0, null);
        jest.advanceTimersByTime(5000);
        expect(spawnFn).toHaveBeenCalledTimes(1);
        jest.useRealTimers();
    });

    it("does not start after dispose (terminal)", () => {
        const child = makeFakeChild();
        const spawnFn = jest.fn(() => child) as unknown as typeof import("child_process").spawn;
        const sup = new ReporterSupervisor({ ...BASE, hubEndpoint: "ws://h", spawnFn });
        sup.dispose();
        sup.start();
        expect(spawnFn).not.toHaveBeenCalled();
    });
});

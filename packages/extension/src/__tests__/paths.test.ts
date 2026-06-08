/**
 * Unit tests for the canonical reporter-socket paths (TS mirror of Rust's
 * `fleet_protocol::paths`).
 */

import * as os from "os";
import * as path from "path";

import {
    FLEET_REPORTER_SOCKET_VAR,
    RUNTIME_SUBDIR,
    runtimeDir,
    reporterSocketPath,
} from "../paths";

describe("FLEET_REPORTER_SOCKET_VAR", () => {
    it("matches the Rust-side env var name exactly", () => {
        expect(FLEET_REPORTER_SOCKET_VAR).toBe("FLEET_REPORTER_SOCKET");
    });
});

describe("runtimeDir()", () => {
    const saved = process.env["XDG_RUNTIME_DIR"];
    afterEach(() => {
        if (saved === undefined) delete process.env["XDG_RUNTIME_DIR"];
        else process.env["XDG_RUNTIME_DIR"] = saved;
    });

    it("uses $XDG_RUNTIME_DIR/fleet when set", () => {
        process.env["XDG_RUNTIME_DIR"] = "/run/user/1000";
        expect(runtimeDir()).toBe(path.join("/run/user/1000", RUNTIME_SUBDIR));
    });

    it("falls back to <os.tmpdir()>/fleet when XDG is unset", () => {
        delete process.env["XDG_RUNTIME_DIR"];
        expect(runtimeDir()).toBe(path.join(os.tmpdir(), RUNTIME_SUBDIR));
    });
});

describe("reporterSocketPath()", () => {
    it("is per-window (keyed by session id) under the runtime dir", () => {
        const a = reporterSocketPath("win-A");
        const b = reporterSocketPath("win-B");
        expect(a).not.toBe(b);
        expect(path.dirname(a)).toBe(runtimeDir());
        expect(path.basename(a)).toBe("reporter-win-A.sock");
    });

    it("sanitizes unsafe session ids into a safe filename (no traversal)", () => {
        const p = reporterSocketPath("../../etc/x y");
        expect(path.basename(p)).not.toContain("/");
        expect(path.basename(p)).not.toContain(" ");
        expect(path.basename(p).endsWith(".sock")).toBe(true);
    });
});

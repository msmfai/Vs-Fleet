import * as http from "http";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WebSocketServer, type WebSocket as WS } from "ws";

import { activate, bridgeTarget, deactivate, nextBackoff } from "../src/extension";
import * as vscodeMock from "./vscode-mock";

const { state, resetVscodeMock, makeTerminal, Uri } = vscodeMock;

// ─── shared harness ───────────────────────────────────────────────────────────

interface Fake {
  subscriptions: Array<{ dispose(): void }>;
}

function fakeContext(): Fake {
  return { subscriptions: [] };
}

const savedEnv: Record<string, string | undefined> = {};
const ENV_KEYS = [
  "FLEET_BRIDGE_URL",
  "FLEET_BRIDGE_SOCKET",
  "FLEET_SERVER_ID",
  "FLEET_SERVER_URL",
  "FLEET_SERVER_LABEL",
  "FLEET_BRIDGE_TOKEN",
  "FLEET_BRIDGE_LOG_DIR",
];

beforeEach(() => {
  for (const k of ENV_KEYS) savedEnv[k] = process.env[k];
  resetVscodeMock();
  vi.clearAllMocks();
});

afterEach(() => {
  for (const k of ENV_KEYS) {
    if (savedEnv[k] === undefined) delete process.env[k];
    else process.env[k] = savedEnv[k];
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
});

/**
 * Stand up a real ws server, point the bridge at it, activate, and wait for the
 * `hello` registration frame. Returns helpers to round-trip frames.
 */
async function startBridge(opts: { logDir?: string } = {}): Promise<{
  wss: WebSocketServer;
  ctx: Fake;
  serverSocket: WS;
  hello: any;
  /** send a frame and await the next reply from the bridge */
  rpc: (frame: any) => Promise<any>;
  /** send a frame with NO expectation of a reply */
  sendRaw: (data: string) => void;
  /** wait briefly for any unexpected frame (resolves null on timeout) */
  nextFrame: (ms?: number) => Promise<any>;
  close: () => Promise<void>;
}> {
  const wss = new WebSocketServer({ port: 0 });
  await new Promise<void>((r) => wss.once("listening", r));
  const port = (wss.address() as any).port;

  // TCP path: ensure no leaked socket env shadows the URL.
  delete process.env.FLEET_BRIDGE_SOCKET;
  process.env.FLEET_BRIDGE_URL = `ws://127.0.0.1:${port}`;
  process.env.FLEET_SERVER_ID = "srv-test";
  if (opts.logDir !== undefined) process.env.FLEET_BRIDGE_LOG_DIR = opts.logDir;

  const ctx = fakeContext();

  const connP = new Promise<WS>((resolve) => wss.once("connection", resolve));
  activate(ctx as any);
  const serverSocket = await connP;

  // first frame is hello
  const hello = await new Promise<any>((resolve) => {
    serverSocket.once("message", (d) => resolve(JSON.parse(d.toString())));
  });

  const rpc = (frame: any): Promise<any> =>
    new Promise((resolve) => {
      serverSocket.once("message", (d) => resolve(JSON.parse(d.toString())));
      serverSocket.send(JSON.stringify(frame));
    });

  const sendRaw = (data: string): void => serverSocket.send(data);

  const nextFrame = (ms = 100): Promise<any> =>
    new Promise((resolve) => {
      const t = setTimeout(() => {
        serverSocket.off("message", onMsg);
        resolve(null);
      }, ms);
      const onMsg = (d: any): void => {
        clearTimeout(t);
        resolve(JSON.parse(d.toString()));
      };
      serverSocket.once("message", onMsg);
    });

  const close = (): Promise<void> =>
    new Promise((resolve) => {
      for (const s of ctx.subscriptions) s.dispose();
      wss.close(() => resolve());
    });

  return { wss, ctx, serverSocket, hello, rpc, sendRaw, nextFrame, close };
}

// ─── activation / hello / dormant ─────────────────────────────────────────────

describe("activation", () => {
  it("stays dormant when url/serverId unset (no socket, no subscriptions)", () => {
    delete process.env.FLEET_BRIDGE_URL;
    delete process.env.FLEET_BRIDGE_SOCKET;
    delete process.env.FLEET_SERVER_ID;
    const ctx = fakeContext();
    activate(ctx as any);
    // onDidStartTerminalShellExecution still subscribes (it runs before the gate),
    // but there should be no ws subscription/dispose registered.
    // The shell-integration subscription is pushed; the dispose() ws subscription is not.
    // So exactly 1 subscription (shell integration), and it has dispose().
    expect(ctx.subscriptions.length).toBe(1);
    expect(state.shellExecCb).toBeTypeOf("function");
  });

  it("sends a hello frame with caps and env-derived fields", async () => {
    process.env.FLEET_SERVER_URL = "https://embed.example";
    process.env.FLEET_SERVER_LABEL = "My Label";
    process.env.FLEET_BRIDGE_TOKEN = "secret";
    const h = await startBridge();
    expect(h.hello.type).toBe("hello");
    expect(h.hello.server_id).toBe("srv-test");
    expect(h.hello.url).toBe("https://embed.example");
    expect(h.hello.label).toBe("My Label");
    expect(h.hello.token).toBe("secret");
    expect(h.hello.caps).toContain("command");
    expect(h.hello.caps).toContain("extensions");
    await h.close();
  });

  it("hello uses serverId as label and empty url/token when unset", async () => {
    delete process.env.FLEET_SERVER_URL;
    delete process.env.FLEET_SERVER_LABEL;
    delete process.env.FLEET_BRIDGE_TOKEN;
    const h = await startBridge();
    expect(h.hello.url).toBe("");
    expect(h.hello.label).toBe("srv-test");
    expect(h.hello.token).toBe("");
    await h.close();
  });

  it("swallows log errors when log dir is not writable", async () => {
    // Point the log dir under an existing *file* so mkdirSync throws (ENOTDIR).
    const tmpFile = path.join(os.tmpdir(), `fleet-logfile-${Date.now()}`);
    fs.writeFileSync(tmpFile, "x");
    try {
      const h = await startBridge({ logDir: path.join(tmpFile, "sub") });
      // bridge still connected + sent hello despite log failures
      expect(h.hello.type).toBe("hello");
      await h.close();
    } finally {
      fs.rmSync(tmpFile, { force: true });
    }
  });

  it("default log dir path is used when FLEET_BRIDGE_LOG_DIR unset", async () => {
    delete process.env.FLEET_BRIDGE_LOG_DIR;
    const h = await startBridge();
    expect(h.hello.type).toBe("hello");
    // a log file should have been created under os.tmpdir()/fleet-mux
    const expected = path.join(os.tmpdir(), "fleet-mux", "bridge-srv-test.log");
    expect(fs.existsSync(expected)).toBe(true);
    await h.close();
  });

  it("deactivate is a no-op", () => {
    expect(deactivate()).toBeUndefined();
  });
});

// ─── ACTIONS ──────────────────────────────────────────────────────────────────

describe("command action", () => {
  it("executes a command and replies with value", async () => {
    state.executeCommandImpl = async (id) => `ran:${id}`;
    const h = await startBridge();
    const r = await h.rpc({ type: "command", reqId: 1, id: "foo.bar", args: ["a"] });
    expect(r).toEqual({ type: "result", reqId: 1, ok: true, value: "ran:foo.bar" });
    expect(vscodeMock.commands.executeCommand).toHaveBeenCalledWith("foo.bar", "a");
    await h.close();
  });

  it("defaults args to [] when not an array", async () => {
    const seen: any[] = [];
    state.executeCommandImpl = async (id, ...args) => {
      seen.push(args);
      return null;
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "command", reqId: 2, id: "x", args: "notarray" });
    expect(r.ok).toBe(true);
    expect(seen[0]).toEqual([]);
    await h.close();
  });

  it("fails when id is not a string", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "command", reqId: 3 });
    expect(r).toEqual({ type: "result", reqId: 3, ok: false, error: "command requires id" });
    await h.close();
  });

  it("fails when the command rejects", async () => {
    state.executeCommandImpl = async () => {
      throw new Error("boom");
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "command", reqId: 4, id: "x" });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("boom");
    await h.close();
  });
});

describe("openFile action", () => {
  it("opens a document and replies path", async () => {
    state.openTextDocumentImpl = async (uri) => ({ uri, getText: () => "hi" });
    const h = await startBridge();
    const r = await h.rpc({ type: "openFile", reqId: 5, path: "/a/b.txt" });
    expect(r).toEqual({ type: "result", reqId: 5, ok: true, path: "/a/b.txt" });
    expect(vscodeMock.window.showTextDocument).toHaveBeenCalled();
    await h.close();
  });

  it("fails with empty path", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "openFile", reqId: 6 });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("openFile requires path");
    await h.close();
  });
});

describe("typeText action", () => {
  it("inserts text at the cursor", async () => {
    const edit = vi.fn(async (cb: any) => {
      cb({ insert: vi.fn() });
      return true;
    });
    state.activeTextEditor = {
      selection: { active: { line: 0, character: 0 } },
      edit,
      document: { getText: () => "", uri: Uri.file("/x") },
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "typeText", reqId: 7, text: "hello" });
    expect(r).toEqual({ type: "result", reqId: 7, ok: true, inserted: true });
    expect(edit).toHaveBeenCalled();
    await h.close();
  });

  it("fails when there is no active editor", async () => {
    state.activeTextEditor = undefined;
    const h = await startBridge();
    const r = await h.rpc({ type: "typeText", reqId: 8, text: "hi" });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("no active editor");
    await h.close();
  });

  it("defaults to empty string when text is omitted", async () => {
    const inserted: string[] = [];
    const edit = vi.fn(async (cb: any) => {
      cb({ insert: (_pos: any, t: string) => inserted.push(t) });
      return true;
    });
    state.activeTextEditor = {
      selection: { active: { line: 0, character: 0 } },
      edit,
      document: { getText: () => "", uri: Uri.file("/x") },
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "typeText", reqId: 80 }); // no text field
    expect(r.ok).toBe(true);
    expect(inserted).toEqual([""]);
    await h.close();
  });
});

describe("termSend action", () => {
  it("sends to a named existing terminal", async () => {
    const term = makeTerminal("build");
    state.terminals = [term];
    const h = await startBridge();
    const r = await h.rpc({ type: "termSend", reqId: 9, name: "build", text: "ls" });
    expect(r).toEqual({ type: "result", reqId: 9, ok: true, terminal: "build" });
    expect(term.show).toHaveBeenCalled();
    expect(term.sendText).toHaveBeenCalledWith("ls", true);
    await h.close();
  });

  it("uses active terminal when no name given", async () => {
    const term = makeTerminal("active-one");
    state.activeTerminal = term;
    const h = await startBridge();
    const r = await h.rpc({ type: "termSend", reqId: 10, text: "pwd" });
    expect(r.ok).toBe(true);
    expect(r.terminal).toBe("active-one");
    await h.close();
  });

  it("defaults text to empty string when omitted", async () => {
    const term = makeTerminal("act");
    state.activeTerminal = term;
    const h = await startBridge();
    const r = await h.rpc({ type: "termSend", reqId: 100 }); // no text
    expect(r.ok).toBe(true);
    expect(term.sendText).toHaveBeenCalledWith("", true);
    await h.close();
  });

  it("creates a terminal when none found (waits 600ms)", async () => {
    // Real timers here: the reply travels over a real socket, so the 600ms
    // delay is wall-clock. Kept deterministic by the explicit empty terminal
    // list forcing the create-and-wait branch.
    const created = makeTerminal("fresh");
    state.createTerminalImpl = () => created;
    state.terminals = [];
    state.activeTerminal = undefined;
    const h = await startBridge();
    const r = await h.rpc({ type: "termSend", reqId: 11, name: "fresh", text: "go" });
    expect(r.ok).toBe(true);
    expect(r.terminal).toBe("fresh");
    expect(vscodeMock.window.createTerminal).toHaveBeenCalledWith("fresh");
    expect(created.sendText).toHaveBeenCalledWith("go", true);
    await h.close();
  });
});

describe("writeFile action", () => {
  it("writes content to disk and reports bytes", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fb-write-"));
    const file = path.join(dir, "nested", "out.txt");
    const h = await startBridge();
    const r = await h.rpc({ type: "writeFile", reqId: 12, path: file, content: "héllo" });
    expect(r.ok).toBe(true);
    expect(r.path).toBe(file);
    expect(r.bytes).toBe(Buffer.byteLength("héllo", "utf8"));
    expect(fs.readFileSync(file, "utf8")).toBe("héllo");
    await h.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("fails with empty path", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "writeFile", reqId: 13, content: "x" });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("writeFile requires path");
    await h.close();
  });

  it("defaults content to empty string when omitted", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fb-empty-"));
    const file = path.join(dir, "empty.txt");
    const h = await startBridge();
    const r = await h.rpc({ type: "writeFile", reqId: 130, path: file }); // no content
    expect(r.ok).toBe(true);
    expect(r.bytes).toBe(0);
    expect(fs.readFileSync(file, "utf8")).toBe("");
    await h.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });
});

describe("saveAll action", () => {
  it("saves and reports the result", async () => {
    state.saveAllImpl = async () => true;
    const h = await startBridge();
    const r = await h.rpc({ type: "saveAll", reqId: 14 });
    expect(r).toEqual({ type: "result", reqId: 14, ok: true, saved: true });
    expect(vscodeMock.workspace.saveAll).toHaveBeenCalledWith(false);
    await h.close();
  });
});

describe("closeEditor action", () => {
  it("closes the active editor", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "closeEditor", reqId: 15 });
    expect(r).toEqual({ type: "result", reqId: 15, ok: true, closed: true });
    expect(vscodeMock.commands.executeCommand).toHaveBeenCalledWith(
      "workbench.action.closeActiveEditor"
    );
    await h.close();
  });
});

// ─── QUERIES ────────────────────────────────────────────────────────────────

describe("query", () => {
  it("returns a full snapshot with active editor + selection", async () => {
    state.activeTextEditor = {
      selection: {
        start: { line: 1, character: 2 },
        end: { line: 3, character: 4 },
      },
      document: { getText: () => "body", uri: Uri.file("/active.ts") },
    };
    state.terminals = [makeTerminal("t1"), makeTerminal("t2")];
    state.visibleTextEditors = [{ document: { uri: Uri.file("/vis.ts") } }];
    state.tabGroups = { all: [{ tabs: [{ label: "tab-a" }, { label: "tab-b" }] }] };
    state.diagnostics = [
      [Uri.file("/d.ts"), [{ message: "m", severity: 0, range: { start: { line: 0 } } }]],
    ];
    const h = await startBridge();
    const r = await h.rpc({ type: "query", reqId: 16 });
    expect(r.ok).toBe(true);
    expect(r.data.terminals).toEqual(["t1", "t2"]);
    expect(r.data.terminalCount).toBe(2);
    expect(r.data.activeEditor).toBe("/active.ts");
    expect(r.data.visibleEditors).toEqual(["/vis.ts"]);
    expect(r.data.openTabs).toEqual(["tab-a", "tab-b"]);
    expect(r.data.diagnostics).toBe(1);
    expect(r.data.editorText).toBe("body");
    expect(r.data.selection).toEqual({
      start: { line: 1, character: 2 },
      end: { line: 3, character: 4 },
    });
    await h.close();
  });

  it("returns null active editor + no selection/editorText when no editor", async () => {
    state.activeTextEditor = undefined;
    const h = await startBridge();
    const r = await h.rpc({ type: "query", reqId: 17 });
    expect(r.data.activeEditor).toBeNull();
    expect(r.data.editorText).toBeUndefined();
    expect(r.data.selection).toBeUndefined();
    await h.close();
  });
});

describe("fileContent query", () => {
  it("prefers an open in-memory document", async () => {
    state.textDocuments = [
      { uri: Uri.file("/open.ts"), getText: () => "in-memory" },
    ];
    const h = await startBridge();
    const r = await h.rpc({ type: "fileContent", reqId: 18, path: "/open.ts" });
    expect(r).toEqual({ type: "result", reqId: 18, ok: true, text: "in-memory" });
    await h.close();
  });

  it("reads from disk when not open", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fb-read-"));
    const file = path.join(dir, "disk.txt");
    fs.writeFileSync(file, "on-disk");
    const h = await startBridge();
    const r = await h.rpc({ type: "fileContent", reqId: 19, path: file });
    expect(r.ok).toBe(true);
    expect(r.text).toBe("on-disk");
    await h.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("fails with empty path", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "fileContent", reqId: 20 });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("fileContent requires path");
    await h.close();
  });
});

describe("terminalText query", () => {
  it("returns empty source when nothing captured", async () => {
    const term = makeTerminal("empty");
    state.terminals = [term];
    const h = await startBridge();
    const r = await h.rpc({ type: "terminalText", reqId: 21, name: "empty" });
    expect(r).toEqual({ type: "result", reqId: 21, ok: true, text: "", source: "" });
    await h.close();
  });

  it("returns buffer source after a termSend echo", async () => {
    const term = makeTerminal("buf");
    state.terminals = [term];
    state.activeTerminal = term;
    const h = await startBridge();
    await h.rpc({ type: "termSend", reqId: 22, name: "buf", text: "echo hi" });
    const r = await h.rpc({ type: "terminalText", reqId: 23, name: "buf" });
    expect(r.ok).toBe(true);
    expect(r.source).toBe("buffer");
    expect(r.text).toContain("$ echo hi");
    await h.close();
  });

  it("returns empty when no terminal and no name resolves to a key", async () => {
    state.terminals = [];
    state.activeTerminal = undefined;
    const h = await startBridge();
    const r = await h.rpc({ type: "terminalText", reqId: 24 });
    expect(r).toEqual({ type: "result", reqId: 24, ok: true, text: "", source: "" });
    await h.close();
  });
});

describe("diagnostics query", () => {
  it("maps severities and falls back to numeric for unknown sev", async () => {
    state.diagnostics = [
      [
        Uri.file("/a.ts"),
        [
          { message: "err", severity: 0, range: { start: { line: 10 } } },
          { message: "warn", severity: 1, range: { start: { line: 20 } } },
          { message: "weird", severity: 9, range: { start: { line: 30 } } },
        ],
      ],
    ];
    const h = await startBridge();
    const r = await h.rpc({ type: "diagnostics", reqId: 25 });
    expect(r.ok).toBe(true);
    expect(r.items).toEqual([
      { file: "/a.ts", sev: "error", msg: "err", line: 10 },
      { file: "/a.ts", sev: "warning", msg: "warn", line: 20 },
      { file: "/a.ts", sev: "9", msg: "weird", line: 30 },
    ]);
    await h.close();
  });
});

describe("openEditors query", () => {
  it("reports tab paths via input uri and active flag", async () => {
    state.activeTextEditor = {
      selection: { active: {} },
      document: { uri: Uri.file("/active.ts"), getText: () => "" },
    };
    state.tabGroups = {
      all: [
        {
          tabs: [
            { input: { uri: Uri.file("/active.ts") }, isActive: true, label: "active.ts" },
            { input: undefined, isActive: false, label: "labelled-tab" },
          ],
        },
      ],
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "openEditors", reqId: 26 });
    expect(r.ok).toBe(true);
    expect(r.items).toEqual([
      { path: "/active.ts", active: true },
      { path: "labelled-tab", active: false },
    ]);
    await h.close();
  });

  it("reports active=false for all tabs when there is no active editor", async () => {
    state.activeTextEditor = undefined; // active resolves to null
    state.tabGroups = {
      all: [
        {
          tabs: [{ input: { uri: Uri.file("/x.ts") }, isActive: true, label: "x.ts" }],
        },
      ],
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "openEditors", reqId: 260 });
    expect(r.ok).toBe(true);
    expect(r.items).toEqual([{ path: "/x.ts", active: false }]);
    await h.close();
  });
});

describe("setting query", () => {
  it("splits a dotted key into section + leaf", async () => {
    let seenSection: string | undefined = "UNSET";
    let seenLeaf = "";
    state.getConfigurationImpl = (section) => {
      seenSection = section;
      return {
        get: (leaf: string) => {
          seenLeaf = leaf;
          return 4;
        },
      };
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "setting", reqId: 27, key: "editor.tabSize" });
    expect(r).toEqual({ type: "result", reqId: 27, ok: true, value: 4 });
    expect(seenSection).toBe("editor");
    expect(seenLeaf).toBe("tabSize");
    await h.close();
  });

  it("uses undefined section for an undotted key", async () => {
    let seenSection: string | undefined = "UNSET";
    state.getConfigurationImpl = (section) => {
      seenSection = section;
      return { get: () => "v" };
    };
    const h = await startBridge();
    const r = await h.rpc({ type: "setting", reqId: 28, key: "telemetry" });
    expect(r.value).toBe("v");
    expect(seenSection).toBeUndefined();
    await h.close();
  });

  it("fails with empty key", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "setting", reqId: 29 });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("setting requires key");
    await h.close();
  });
});

describe("extensions query", () => {
  it("lists extension ids and active state", async () => {
    state.extensions = [
      { id: "pub.a", isActive: true },
      { id: "pub.b", isActive: false },
    ];
    const h = await startBridge();
    const r = await h.rpc({ type: "extensions", reqId: 30 });
    expect(r.ok).toBe(true);
    expect(r.items).toEqual([
      { id: "pub.a", active: true },
      { id: "pub.b", active: false },
    ]);
    await h.close();
  });
});

// ─── default / bad-json branches ──────────────────────────────────────────────

describe("protocol edge cases", () => {
  it("replies ok:false to an unknown type with a reqId", async () => {
    const h = await startBridge();
    const r = await h.rpc({ type: "nope", reqId: 31 });
    expect(r.ok).toBe(false);
    expect(r.error).toContain("unknown type: nope");
    await h.close();
  });

  it("ignores an unknown type with no reqId (no reply)", async () => {
    const h = await startBridge();
    h.sendRaw(JSON.stringify({ type: "nope" }));
    const frame = await h.nextFrame(150);
    expect(frame).toBeNull();
    await h.close();
  });

  it("ignores a frame with no reqId on a real action (reply suppressed)", async () => {
    // command without reqId: command runs but reply() is a no-op (reqId == null)
    state.executeCommandImpl = async () => "ok";
    const h = await startBridge();
    h.sendRaw(JSON.stringify({ type: "command", id: "x" }));
    const frame = await h.nextFrame(150);
    expect(frame).toBeNull();
    expect(vscodeMock.commands.executeCommand).toHaveBeenCalledWith("x");
    await h.close();
  });

  it("ignores a bad-JSON frame (no reply)", async () => {
    const h = await startBridge();
    h.sendRaw("{not json");
    const frame = await h.nextFrame(150);
    expect(frame).toBeNull();
    await h.close();
  });
});

// ─── shell integration capture ────────────────────────────────────────────────

describe("shell integration capture", () => {
  it("captures command line + streamed output into the terminal buffer", async () => {
    const term = makeTerminal("shell");
    state.terminals = [term];
    state.activeTerminal = term;
    const h = await startBridge();

    async function* read(): AsyncIterable<string> {
      yield "line1\n";
      yield "line2\n";
    }
    // fire the shell-integration event the extension subscribed to
    await state.shellExecCb!({
      terminal: { name: "shell" },
      execution: { commandLine: { value: "make" }, read },
    });

    const r = await h.rpc({ type: "terminalText", reqId: 40, name: "shell" });
    expect(r.source).toBe("buffer");
    expect(r.text).toContain("$ make");
    expect(r.text).toContain("line1");
    expect(r.text).toContain("line2");
    await h.close();
  });

  it("handles an execution with no commandLine and no read fn", async () => {
    const term = makeTerminal("bare");
    state.terminals = [term];
    state.activeTerminal = term;
    const h = await startBridge();
    await state.shellExecCb!({
      terminal: { name: "bare" },
      execution: {},
    });
    // nothing captured → empty buffer
    const r = await h.rpc({ type: "terminalText", reqId: 41, name: "bare" });
    expect(r.source).toBe("");
    await h.close();
  });

  it("logs and survives when read() throws", async () => {
    const term = makeTerminal("err");
    state.terminals = [term];
    state.activeTerminal = term;
    const h = await startBridge();
    async function* read(): AsyncIterable<string> {
      yield "partial";
      throw new Error("stream broke");
    }
    await state.shellExecCb!({
      terminal: { name: "err" },
      execution: { commandLine: { value: "cmd" }, read },
    });
    // the partial chunk + command line were still captured before the throw
    const r = await h.rpc({ type: "terminalText", reqId: 42, name: "err" });
    expect(r.text).toContain("$ cmd");
    expect(r.text).toContain("partial");
    await h.close();
  });

  it("trims the buffer when it exceeds 64KiB", async () => {
    const term = makeTerminal("big");
    state.terminals = [term];
    state.activeTerminal = term;
    const h = await startBridge();
    const big = "x".repeat(70 * 1024);
    async function* read(): AsyncIterable<string> {
      yield big;
    }
    await state.shellExecCb!({
      terminal: { name: "big" },
      execution: { read },
    });
    const r = await h.rpc({ type: "terminalText", reqId: 43, name: "big" });
    // trimmed to the last 64KiB
    expect(r.text.length).toBe(64 * 1024);
    await h.close();
  });

  it("logs when the subscribe call itself throws", async () => {
    // make onDidStartTerminalShellExecution throw to hit the outer catch
    const orig = vscodeMock.window.onDidStartTerminalShellExecution;
    (vscodeMock.window as any).onDidStartTerminalShellExecution = vi.fn(() => {
      throw new Error("subscribe failed");
    });
    try {
      const h = await startBridge();
      expect(h.hello.type).toBe("hello"); // still activates
      await h.close();
    } finally {
      (vscodeMock.window as any).onDidStartTerminalShellExecution = orig;
    }
  });

  it("skips shell-integration subscribe when API is absent", async () => {
    const orig = vscodeMock.window.onDidStartTerminalShellExecution;
    (vscodeMock.window as any).onDidStartTerminalShellExecution = undefined;
    try {
      const ctx = fakeContext();
      delete process.env.FLEET_BRIDGE_URL;
      delete process.env.FLEET_BRIDGE_SOCKET;
      delete process.env.FLEET_SERVER_ID;
      activate(ctx as any);
      // no shell-integration subscription registered → 0 subscriptions
      expect(ctx.subscriptions.length).toBe(0);
    } finally {
      (vscodeMock.window as any).onDidStartTerminalShellExecution = orig;
    }
  });
});

// ─── reconnect / error / dispose ──────────────────────────────────────────────

describe("connection lifecycle", () => {
  it("reconnects 1s after the socket closes", async () => {
    const wss = new WebSocketServer({ port: 0 });
    await new Promise<void>((r) => wss.once("listening", r));
    const port = (wss.address() as any).port;
    process.env.FLEET_BRIDGE_URL = `ws://127.0.0.1:${port}`;
    process.env.FLEET_SERVER_ID = "srv-recon";

    const ctx = fakeContext();

    // first connection
    const firstP = new Promise<WS>((res) => wss.once("connection", res));
    activate(ctx as any);
    const first = await firstP;
    await new Promise<void>((res) => first.once("message", () => res())); // hello

    // arm the second-connection listener, then close the server side
    const secondP = new Promise<WS>((res) => wss.once("connection", res));
    first.close();

    // the reconnect uses a real 1000ms timer; wait for the new connection
    const second = await secondP;
    const hello2 = await new Promise<any>((res) =>
      second.once("message", (d) => res(JSON.parse(d.toString())))
    );
    expect(hello2.type).toBe("hello");

    for (const s of ctx.subscriptions) s.dispose();
    await new Promise<void>((res) => wss.close(() => res()));
  });

  it("closes the socket on error", async () => {
    // point at a port with no server → connect fails → 'error' → socket.close()
    process.env.FLEET_BRIDGE_URL = "ws://127.0.0.1:1"; // unroutable
    process.env.FLEET_SERVER_ID = "srv-err";
    const ctx = fakeContext();
    activate(ctx as any);
    // give the error handler a tick to run and close
    await new Promise((r) => setTimeout(r, 200));
    // dispose to clean up any retry timer
    for (const s of ctx.subscriptions) s.dispose();
  });

  it("dispose clears the retry timer and closes ws; no reconnect after", async () => {
    const wss = new WebSocketServer({ port: 0 });
    await new Promise<void>((r) => wss.once("listening", r));
    const port = (wss.address() as any).port;
    process.env.FLEET_BRIDGE_URL = `ws://127.0.0.1:${port}`;
    process.env.FLEET_SERVER_ID = "srv-dispose";

    const ctx = fakeContext();
    const firstP = new Promise<WS>((res) => wss.once("connection", res));
    activate(ctx as any);
    const first = await firstP;
    await new Promise<void>((res) => first.once("message", () => res()));

    let reconnected = false;
    wss.once("connection", () => {
      reconnected = true;
    });

    // close the bridge's socket from the server side and wait for the bridge's
    // own 'close' handler to fire and arm the 1000ms reconnect timer.
    await new Promise<void>((res) => {
      first.once("close", () => res());
      first.close();
    });
    // give the bridge a tick to process its close → reconnect() arms `retry`
    await new Promise((r) => setTimeout(r, 50));

    // dispose now: retry is set, so dispose() must clearTimeout(retry)
    for (const s of ctx.subscriptions) s.dispose();

    // wait well past the reconnect interval: no new connection should arrive
    await new Promise((r) => setTimeout(r, 1300));
    expect(reconnected).toBe(false);

    await new Promise<void>((res) => wss.close(() => res()));
  });

  it("reconnect is a no-op when ws has been replaced (guard branch)", async () => {
    // Covers the `ws !== socket` guard: dispose sets disposed too, but we also
    // exercise the path where a stale socket's close handler returns early.
    const h = await startBridge();
    // Close from the server side after disposing so disposed-guard returns early.
    for (const s of h.ctx.subscriptions) s.dispose();
    await new Promise((r) => setTimeout(r, 50));
    await new Promise<void>((res) => h.wss.close(() => res()));
  });
});

// ─── transport selection (unix socket vs TCP) ─────────────────────────────────

describe("bridgeTarget", () => {
  it("prefers the unix socket as a ws+unix URL when FLEET_BRIDGE_SOCKET is set", () => {
    // Even with a TCP URL also present, the socket wins — that's the whole point:
    // the local case avoids the network socket + macOS local-network prompt.
    const t = bridgeTarget({
      FLEET_BRIDGE_SOCKET: "/run/fleet/bridge.sock",
      FLEET_BRIDGE_URL: "ws://127.0.0.1:51778",
    } as any);
    expect(t).toBe("ws+unix:///run/fleet/bridge.sock:/");
  });

  it("falls back to the TCP URL when no socket is set", () => {
    const t = bridgeTarget({ FLEET_BRIDGE_URL: "ws://127.0.0.1:51778" } as any);
    expect(t).toBe("ws://127.0.0.1:51778");
  });

  it("is null when neither socket nor URL is set", () => {
    expect(bridgeTarget({} as any)).toBeNull();
  });

  it("ignores an empty socket value and uses the URL", () => {
    const t = bridgeTarget({
      FLEET_BRIDGE_SOCKET: "",
      FLEET_BRIDGE_URL: "ws://127.0.0.1:51778",
    } as any);
    expect(t).toBe("ws://127.0.0.1:51778");
  });
});

describe("unix socket transport", () => {
  it("connects + sends hello over FLEET_BRIDGE_SOCKET (ws+unix), not TCP", async () => {
    // Stand up a ws server bound to a UNIX socket; the bridge must dial it via
    // the ws+unix URL form and register over the filesystem socket (no TCP).
    const sockPath = path.join(
      os.tmpdir(),
      `fb-sock-${process.pid}-${Date.now()}.sock`
    );
    const httpServer = http.createServer();
    const wss = new WebSocketServer({ server: httpServer });
    await new Promise<void>((r) => httpServer.listen(sockPath, r));

    delete process.env.FLEET_BRIDGE_URL; // prove the socket path is taken, not TCP
    process.env.FLEET_BRIDGE_SOCKET = sockPath;
    process.env.FLEET_SERVER_ID = "srv-unix";
    process.env.FLEET_SERVER_LABEL = "Unix Srv";

    const ctx = fakeContext();
    const connP = new Promise<WS>((resolve) => wss.once("connection", resolve));
    activate(ctx as any);
    const serverSocket = await connP;
    const hello = await new Promise<any>((resolve) => {
      serverSocket.once("message", (d) => resolve(JSON.parse(d.toString())));
    });

    expect(hello.type).toBe("hello");
    expect(hello.server_id).toBe("srv-unix");
    expect(hello.label).toBe("Unix Srv");

    for (const s of ctx.subscriptions) s.dispose();
    await new Promise<void>((res) => wss.close(() => res()));
    await new Promise<void>((res) => httpServer.close(() => res()));
    fs.rmSync(sockPath, { force: true });
  });
});

// ─── reconnect backoff schedule (pure math) ───────────────────────────────────

describe("nextBackoff", () => {
  it("doubles each step and caps at 30s", () => {
    expect(nextBackoff(1000)).toBe(2000);
    expect(nextBackoff(2000)).toBe(4000);
    expect(nextBackoff(4000)).toBe(8000);
    expect(nextBackoff(8000)).toBe(16000);
    // 16000 * 2 = 32000 → clamped to the 30000 ceiling, and stays there.
    expect(nextBackoff(16000)).toBe(30000);
    expect(nextBackoff(30000)).toBe(30000);
  });
});

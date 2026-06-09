/**
 * Fleet Bridge — the command bridge that lets the Fleet multiplexer drive this
 * VS Code. On activation it connects back to Fleet's bridge WS server
 * (`FLEET_BRIDGE_URL`), registers as this server (`FLEET_SERVER_ID`), and runs
 * `vscode.commands.executeCommand(id)` for every command Fleet forwards from its
 * native menu. This is the only reliable way to forward commands into a web VS
 * Code (synthetic keystrokes are untrusted); the extension runs in the server's
 * Node extension host, so `process.env` + `ws` are available.
 *
 * ─── BRIDGE WIRE PROTOCOL (frozen — harness side must match this) ──────────────
 *
 * Every server→bridge frame carries a numeric `reqId`. The bridge always replies
 * with `{ type:"result", reqId, ok:boolean, ... }`. On failure: `ok:false` +
 * `error:string`. On success the payload fields are action/query specific (below).
 *
 * HELLO (bridge→server, on connect; not a reply):
 *   { type:"hello", server_id, url, label, caps:string[] }
 *     caps = the capability tokens this bridge supports, so the harness can gate
 *     behaviours via Behaviour.needs[]. Current caps (see CAPS const):
 *       "command" "query" "openFile" "typeText" "termSend" "writeFile"
 *       "saveAll" "closeEditor" "fileContent" "terminalText" "diagnostics"
 *       "openEditors" "setting" "extensions" "editorText" "selection"
 *
 * ACTIONS (server→bridge → reply { type:"result", reqId, ok, ... }):
 *   { type:"command",   id:string, args?:any[] }      → { value }   (DONE; kept)
 *   { type:"openFile",  path:string }                 → { path }    opens doc in editor
 *   { type:"typeText",  text:string }                 → { inserted:true }  insert at cursor
 *   { type:"termSend",  name?:string, text:string }   → { terminal } sendText(+\n) to a
 *                                                        terminal (named, else active,
 *                                                        else a freshly created one)
 *   { type:"writeFile", path:string, content:string } → { path, bytes }  write to disk
 *   { type:"saveAll" }                                → { saved:true }    save all dirty
 *   { type:"closeEditor" }                            → { closed:true }   close active editor
 *
 * QUERIES (server→bridge → reply { type:"result", reqId, ok, ... }):
 *   { type:"query" }                          → { data:Snapshot }   (DONE; kept)
 *   { type:"fileContent", path:string }       → { text }   prefers open-doc text, else disk
 *   { type:"terminalText", name?:string }     → { text, source }   terminal buffer text;
 *                                                source = "buffer" (populated) | "" (empty)
 *   { type:"diagnostics", detailed?:true }    → { items:[{file,sev,msg,line}] }
 *   { type:"openEditors" }                    → { items:[{path,active}] }
 *   { type:"setting", key:string }            → { value }   config.get(key)
 *   { type:"extensions" }                     → { items:[{id,active}] }
 *
 * Snapshot (from query):
 *   { terminals:string[], terminalCount:number, activeEditor:string|null,
 *     visibleEditors:string[], openTabs:string[], diagnostics:number,
 *     editorText?:string, selection?:{start:{line,character},end:{line,character}} }
 */

import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import * as vscode from "vscode";
import WebSocket from "ws";

// Capability tokens advertised in the `hello` frame. Behaviours gate on these
// via `needs:[...]`; anything not present here makes a behaviour SKIP cleanly.
const CAPS = [
  "command",
  "query",
  "openFile",
  "typeText",
  "termSend",
  "writeFile",
  "saveAll",
  "closeEditor",
  "fileContent",
  "terminalText",
  "diagnostics",
  "openEditors",
  "setting",
  "extensions",
  "editorText",
  "selection",
];

export function activate(context: vscode.ExtensionContext): void {
  const url = process.env.FLEET_BRIDGE_URL;
  const serverId = process.env.FLEET_SERVER_ID;

  // Diagnostic log file so Fleet can verify activation/receipt/execution.
  const logPath = path.join(os.tmpdir(), "fleet-mux", `bridge-${serverId || "unknown"}.log`);
  const log = (msg: string): void => {
    try {
      fs.mkdirSync(path.dirname(logPath), { recursive: true });
      fs.appendFileSync(logPath, `${new Date().toISOString()} ${msg}\n`);
    } catch {
      /* best-effort */
    }
  };
  log(`activate: url=${url} serverId=${serverId}`);

  // ─── Terminal buffer capture ────────────────────────────────────────────────
  // VS Code exposes no API to read a terminal's scrollback. We approximate it via
  // shell integration: capture the command lines + their output streams as they
  // execute and keep a per-terminal rolling buffer keyed by terminal name. This
  // is the `terminalText` source of truth ("shellIntegration"); when shell
  // integration is unavailable we fall back to whatever raw text we managed to
  // capture ("captured"), else empty.
  const termBuffers = new Map<string, string>();
  const appendTermBuf = (name: string, chunk: string): void => {
    const prev = termBuffers.get(name) ?? "";
    let next = prev + chunk;
    if (next.length > 64 * 1024) next = next.slice(next.length - 64 * 1024);
    termBuffers.set(name, next);
  };

  // Shell-integration command execution: read the output stream when available.
  const winAny = vscode.window as unknown as {
    onDidStartTerminalShellExecution?: (
      cb: (e: {
        terminal: vscode.Terminal;
        execution: {
          commandLine?: { value?: string };
          read?: () => AsyncIterable<string>;
        };
      }) => void
    ) => vscode.Disposable;
  };
  if (typeof winAny.onDidStartTerminalShellExecution === "function") {
    try {
      context.subscriptions.push(
        winAny.onDidStartTerminalShellExecution(async (e) => {
          const name = e.terminal.name;
          const cmd = e.execution?.commandLine?.value;
          if (cmd) appendTermBuf(name, `$ ${cmd}\n`);
          try {
            if (typeof e.execution?.read === "function") {
              for await (const chunk of e.execution.read()) {
                appendTermBuf(name, chunk);
              }
            }
          } catch (err) {
            log(`shellExec read err: ${err}`);
          }
        })
      );
      log("shell integration: subscribed");
    } catch (err) {
      log(`shell integration subscribe err: ${err}`);
    }
  }

  if (!url || !serverId) {
    // Not launched by Fleet — stay dormant (pure pass-through, never intrusive).
    return;
  }

  let ws: WebSocket | null = null;
  let disposed = false;
  let retry: ReturnType<typeof setTimeout> | null = null;

  const connect = (): void => {
    if (disposed) return;
    ws = new WebSocket(url);

    ws.on("open", () => {
      // Phone home: the server PUSHES its registration to Fleet (id + the URL
      // Fleet should embed + a label + the capabilities it supports). Fleet
      // never pulls a server list.
      const registration = {
        type: "hello",
        server_id: serverId,
        url: process.env.FLEET_SERVER_URL || "",
        label: process.env.FLEET_SERVER_LABEL || serverId,
        caps: CAPS,
      };
      log(`ws open → hello ${JSON.stringify(registration)}`);
      ws?.send(JSON.stringify(registration));
    });

    const send = (obj: unknown): void => ws?.send(JSON.stringify(obj));
    const reply = (reqId: unknown, ok: boolean, extra: Record<string, unknown> = {}): void => {
      if (reqId != null) send({ type: "result", reqId, ok, ...extra });
    };
    const fail = (reqId: unknown, e: unknown): void => reply(reqId, false, { error: String(e) });

    // An observation of this VS Code's state — the testable "what happened"
    // surface (terminals, editors, tabs, diagnostics). Read straight from the
    // extension API, so it's exact, not scraped from pixels.
    const snapshot = (): Record<string, unknown> => {
      const ed = vscode.window.activeTextEditor;
      const sel = ed?.selection;
      return {
        terminals: vscode.window.terminals.map((t) => t.name),
        terminalCount: vscode.window.terminals.length,
        activeEditor: ed?.document.uri.fsPath ?? null,
        visibleEditors: vscode.window.visibleTextEditors.map((e) => e.document.uri.fsPath),
        openTabs: vscode.window.tabGroups.all.flatMap((g) => g.tabs.map((t) => t.label)),
        diagnostics: vscode.languages.getDiagnostics().reduce((n, [, ds]) => n + ds.length, 0),
        editorText: ed ? ed.document.getText() : undefined,
        selection: sel
          ? {
              start: { line: sel.start.line, character: sel.start.character },
              end: { line: sel.end.line, character: sel.end.character },
            }
          : undefined,
      };
    };

    // ─── helpers shared by actions/queries ───────────────────────────────────
    const findTerminal = (name?: string): vscode.Terminal | undefined => {
      if (name) return vscode.window.terminals.find((t) => t.name === name);
      return vscode.window.activeTerminal ?? vscode.window.terminals[0];
    };

    ws.on("message", (data: WebSocket.RawData) => {
      let msg: Record<string, unknown>;
      try {
        msg = JSON.parse(data.toString());
      } catch (e) {
        log(`bad frame: ${e}`);
        return;
      }
      const reqId = msg.reqId;
      const type = msg.type;

      // Run an async handler and funnel errors to a {ok:false} reply.
      const handle = (fn: () => Promise<void> | void): void => {
        try {
          Promise.resolve(fn()).catch((e) => {
            log(`handler ERR ${type}: ${e}`);
            fail(reqId, e);
          });
        } catch (e) {
          log(`handler ERR ${type}: ${e}`);
          fail(reqId, e);
        }
      };

      switch (type) {
        // ── ACTIONS ──────────────────────────────────────────────────────────
        case "command": {
          if (typeof msg.id !== "string") return fail(reqId, "command requires id");
          const args = Array.isArray(msg.args) ? msg.args : [];
          log(`command recv: ${msg.id}`);
          vscode.commands.executeCommand(msg.id as string, ...args).then(
            (value) => {
              log(`command ok: ${msg.id}`);
              reply(reqId, true, { value });
            },
            (e) => {
              log(`command ERR: ${msg.id} ${e}`);
              fail(reqId, e);
            }
          );
          break;
        }

        case "openFile":
          handle(async () => {
            const p = String(msg.path ?? "");
            if (!p) throw new Error("openFile requires path");
            const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(p));
            await vscode.window.showTextDocument(doc, { preview: false });
            reply(reqId, true, { path: doc.uri.fsPath });
          });
          break;

        case "typeText":
          handle(async () => {
            const text = String(msg.text ?? "");
            const ed = vscode.window.activeTextEditor;
            if (!ed) throw new Error("no active editor");
            const ok = await ed.edit((b) => b.insert(ed.selection.active, text));
            reply(reqId, true, { inserted: ok });
          });
          break;

        case "termSend":
          handle(async () => {
            const text = String(msg.text ?? "");
            const name = msg.name != null ? String(msg.name) : undefined;
            let term = findTerminal(name);
            if (!term) {
              term = vscode.window.createTerminal(name);
              // give the shell a moment to come up before sending
              await new Promise((r) => setTimeout(r, 600));
            }
            term.show();
            // record what we sent so terminalText has something even without
            // shell integration.
            appendTermBuf(term.name, `$ ${text}\n`);
            term.sendText(text, true);
            reply(reqId, true, { terminal: term.name });
          });
          break;

        case "writeFile":
          handle(async () => {
            const p = String(msg.path ?? "");
            const content = String(msg.content ?? "");
            if (!p) throw new Error("writeFile requires path");
            fs.mkdirSync(path.dirname(p), { recursive: true });
            fs.writeFileSync(p, content, "utf8");
            reply(reqId, true, { path: p, bytes: Buffer.byteLength(content, "utf8") });
          });
          break;

        case "saveAll":
          handle(async () => {
            const saved = await vscode.workspace.saveAll(false);
            reply(reqId, true, { saved });
          });
          break;

        case "closeEditor":
          handle(async () => {
            await vscode.commands.executeCommand("workbench.action.closeActiveEditor");
            reply(reqId, true, { closed: true });
          });
          break;

        // ── QUERIES ──────────────────────────────────────────────────────────
        case "query":
          log(`query recv: reqId=${reqId}`);
          reply(reqId, true, { data: snapshot() });
          break;

        case "fileContent":
          handle(async () => {
            const p = String(msg.path ?? "");
            if (!p) throw new Error("fileContent requires path");
            // prefer the in-memory doc (reflects unsaved edits), else read disk.
            const open = vscode.workspace.textDocuments.find((d) => d.uri.fsPath === p);
            if (open) {
              reply(reqId, true, { text: open.getText() });
              return;
            }
            const text = fs.readFileSync(p, "utf8");
            reply(reqId, true, { text });
          });
          break;

        case "terminalText":
          handle(async () => {
            const name = msg.name != null ? String(msg.name) : undefined;
            const term = findTerminal(name);
            const key = term?.name ?? name ?? "";
            const buf = key ? termBuffers.get(key) ?? "" : "";
            // buffer is a mix of shell-integration output and our own termSend
            // echoes; report "buffer" when populated, "" when we captured nothing.
            const source = buf ? "buffer" : "";
            reply(reqId, true, { text: buf, source });
          });
          break;

        case "diagnostics":
          handle(async () => {
            const items: Array<{ file: string; sev: string; msg: string; line: number }> = [];
            const sevName = (s: vscode.DiagnosticSeverity): string =>
              ["error", "warning", "info", "hint"][s] ?? String(s);
            for (const [uri, ds] of vscode.languages.getDiagnostics()) {
              for (const d of ds) {
                items.push({
                  file: uri.fsPath,
                  sev: sevName(d.severity),
                  msg: d.message,
                  line: d.range.start.line,
                });
              }
            }
            reply(reqId, true, { items });
          });
          break;

        case "openEditors":
          handle(async () => {
            const active = vscode.window.activeTextEditor?.document.uri.fsPath ?? null;
            const items = vscode.window.tabGroups.all.flatMap((g) =>
              g.tabs.map((t) => {
                const input = t.input as { uri?: vscode.Uri } | undefined;
                const p = input?.uri?.fsPath ?? t.label;
                return { path: p, active: p === active && t.isActive };
              })
            );
            reply(reqId, true, { items });
          });
          break;

        case "setting":
          handle(async () => {
            const key = String(msg.key ?? "");
            if (!key) throw new Error("setting requires key");
            // split into section + leaf so getConfiguration resolves scopes right.
            const dot = key.lastIndexOf(".");
            const section = dot >= 0 ? key.slice(0, dot) : "";
            const leaf = dot >= 0 ? key.slice(dot + 1) : key;
            const value = vscode.workspace.getConfiguration(section || undefined).get(leaf);
            reply(reqId, true, { value });
          });
          break;

        case "extensions":
          handle(async () => {
            const items = vscode.extensions.all.map((e) => ({
              id: e.id,
              active: e.isActive,
            }));
            reply(reqId, true, { items });
          });
          break;

        default:
          // unknown frame type — ignore (don't reply; could be a no-reqId notice).
          if (reqId != null) fail(reqId, `unknown type: ${String(type)}`);
          break;
      }
    });

    const reconnect = (): void => {
      if (disposed || retry) return;
      retry = setTimeout(() => {
        retry = null;
        connect();
      }, 1000);
    };
    ws.on("close", reconnect);
    ws.on("error", () => ws?.close());
  };

  connect();

  context.subscriptions.push({
    dispose() {
      disposed = true;
      if (retry) clearTimeout(retry);
      ws?.close();
    },
  });
}

export function deactivate(): void {}

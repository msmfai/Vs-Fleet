/**
 * Fleet Bridge — the command bridge that lets the Fleet multiplexer drive this
 * VS Code. On activation it connects back to Fleet's bridge WS server
 * (`FLEET_BRIDGE_URL`), registers as this server (`FLEET_SERVER_ID`), and runs
 * `vscode.commands.executeCommand(id)` for every command Fleet forwards from its
 * native menu. This is the only reliable way to forward commands into a web VS
 * Code (synthetic keystrokes are untrusted); the extension runs in the server's
 * Node extension host, so `process.env` + `ws` are available.
 */

import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import * as vscode from "vscode";
import WebSocket from "ws";

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
      // Fleet should embed + a label). Fleet never pulls a server list.
      const registration = {
        type: "hello",
        server_id: serverId,
        url: process.env.FLEET_SERVER_URL || "",
        label: process.env.FLEET_SERVER_LABEL || serverId,
      };
      log(`ws open → hello ${JSON.stringify(registration)}`);
      ws?.send(JSON.stringify(registration));
    });

    const send = (obj: unknown): void => ws?.send(JSON.stringify(obj));

    // An observation of this VS Code's state — the testable "what happened"
    // surface (terminals, editors, tabs, diagnostics). Read straight from the
    // extension API, so it's exact, not scraped from pixels.
    const snapshot = (): Record<string, unknown> => ({
      terminals: vscode.window.terminals.map((t) => t.name),
      terminalCount: vscode.window.terminals.length,
      activeEditor: vscode.window.activeTextEditor?.document.uri.fsPath ?? null,
      visibleEditors: vscode.window.visibleTextEditors.map((e) => e.document.uri.fsPath),
      openTabs: vscode.window.tabGroups.all.flatMap((g) => g.tabs.map((t) => t.label)),
      diagnostics: vscode.languages
        .getDiagnostics()
        .reduce((n, [, ds]) => n + ds.length, 0),
    });

    ws.on("message", (data: WebSocket.RawData) => {
      try {
        const msg = JSON.parse(data.toString());
        // ACT: run a command; reply with a result if a reqId was given.
        if (msg && msg.type === "command" && typeof msg.id === "string") {
          const args = Array.isArray(msg.args) ? msg.args : [];
          log(`command recv: ${msg.id}`);
          vscode.commands.executeCommand(msg.id, ...args).then(
            (value) => {
              log(`command ok: ${msg.id}`);
              if (msg.reqId != null) send({ type: "result", reqId: msg.reqId, ok: true, value });
            },
            (e) => {
              log(`command ERR: ${msg.id} ${e}`);
              if (msg.reqId != null)
                send({ type: "result", reqId: msg.reqId, ok: false, error: String(e) });
            }
          );
        } else if (msg && msg.type === "query") {
          // OBSERVE: reply with a state snapshot.
          log(`query recv: reqId=${msg.reqId}`);
          send({ type: "result", reqId: msg.reqId, ok: true, data: snapshot() });
        }
      } catch (e) {
        log(`bad frame: ${e}`);
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

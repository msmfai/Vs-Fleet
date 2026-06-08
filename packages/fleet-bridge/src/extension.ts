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

    ws.on("message", (data: WebSocket.RawData) => {
      try {
        const msg = JSON.parse(data.toString());
        if (msg && msg.type === "command" && typeof msg.id === "string") {
          const args = Array.isArray(msg.args) ? msg.args : [];
          log(`command recv: ${msg.id}`);
          vscode.commands.executeCommand(msg.id, ...args).then(
            () => log(`command ok: ${msg.id}`),
            (e) => log(`command ERR: ${msg.id} ${e}`)
          );
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

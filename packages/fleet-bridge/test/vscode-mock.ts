/**
 * Test double for the `vscode` module. extension.ts imports `* as vscode`,
 * and vitest aliases "vscode" → this file. Everything here is mutable so tests
 * can drive the extension's view of editor/terminal/workspace state.
 */
import { vi } from "vitest";

// ─── mutable state, reset by resetVscodeMock() ────────────────────────────────
export const state: {
  activeTextEditor: any;
  terminals: any[];
  activeTerminal: any;
  visibleTextEditors: any[];
  tabGroups: { all: any[] };
  textDocuments: any[];
  diagnostics: any[]; // [Uri, Diagnostic[]][]
  extensions: any[];
  // controllable handler/returns
  shellExecCb: ((e: any) => void) | null;
  executeCommandImpl: (id: string, ...args: any[]) => Promise<any>;
  openTextDocumentImpl: (uri: any) => Promise<any>;
  showTextDocumentImpl: (doc: any, opts: any) => Promise<any>;
  saveAllImpl: (includeUntitled: boolean) => Promise<boolean>;
  getConfigurationImpl: (section?: string) => { get: (leaf: string) => any };
  createTerminalImpl: (name?: string) => any;
} = {} as any;

export function resetVscodeMock(): void {
  state.activeTextEditor = undefined;
  state.terminals = [];
  state.activeTerminal = undefined;
  state.visibleTextEditors = [];
  state.tabGroups = { all: [] };
  state.textDocuments = [];
  state.diagnostics = [];
  state.extensions = [];
  state.shellExecCb = null;
  state.executeCommandImpl = async () => undefined;
  state.openTextDocumentImpl = async (uri: any) => ({
    uri,
    getText: () => "",
  });
  state.showTextDocumentImpl = async () => undefined;
  state.saveAllImpl = async () => true;
  state.getConfigurationImpl = () => ({ get: () => undefined });
  state.createTerminalImpl = (name?: string) => makeTerminal(name ?? "created");
}

export function makeTerminal(name: string): any {
  return {
    name,
    show: vi.fn(),
    sendText: vi.fn(),
  };
}

resetVscodeMock();

// ─── the vscode namespace surface ─────────────────────────────────────────────
export const window = {
  get activeTextEditor() {
    return state.activeTextEditor;
  },
  get terminals() {
    return state.terminals;
  },
  get activeTerminal() {
    return state.activeTerminal;
  },
  get visibleTextEditors() {
    return state.visibleTextEditors;
  },
  get tabGroups() {
    return state.tabGroups;
  },
  createTerminal: vi.fn((name?: string) => state.createTerminalImpl(name)),
  showTextDocument: vi.fn((doc: any, opts: any) => state.showTextDocumentImpl(doc, opts)),
  onDidStartTerminalShellExecution: vi.fn((cb: (e: any) => void) => {
    state.shellExecCb = cb;
    return { dispose: vi.fn() };
  }),
};

export const commands = {
  executeCommand: vi.fn((id: string, ...args: any[]) => state.executeCommandImpl(id, ...args)),
};

export const workspace = {
  openTextDocument: vi.fn((uri: any) => state.openTextDocumentImpl(uri)),
  saveAll: vi.fn((includeUntitled: boolean) => state.saveAllImpl(includeUntitled)),
  get textDocuments() {
    return state.textDocuments;
  },
  getConfiguration: vi.fn((section?: string) => state.getConfigurationImpl(section)),
};

export const languages = {
  getDiagnostics: vi.fn(() => state.diagnostics),
};

export const extensions = {
  get all() {
    return state.extensions;
  },
};

export const Uri = {
  file: (p: string) => ({
    fsPath: p,
    scheme: "file",
    path: p,
  }),
};

export const DiagnosticSeverity = {
  Error: 0,
  Warning: 1,
  Information: 2,
  Hint: 3,
};

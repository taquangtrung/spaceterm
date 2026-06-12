import * as path from "node:path";

import * as pty from "node-pty";
import * as vscode from "vscode";

import { loadAddon } from "./spaceterm-addon";
import type { SpaceTermTerminal } from "./spaceterm-addon";

// ========================================================================
// Constants
// ========================================================================

const VIEW_TYPE = "spaceterm.terminal";
const ADDON_RELATIVE = path.join(
  "..",
  "..",
  "crates",
  "bindings",
  "spaceterm.node",
);
const DEFAULT_SHELL =
  process.platform === "win32"
    ? "powershell.exe"
    : (process.env.SHELL ?? "/bin/bash");
const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;

// ========================================================================
// Data Structures
// ========================================================================

// Messages the webview sends back to the extension host.
type HostToExtMessage =
  | { type: "input"; data: string }
  | { type: "resize"; cols: number; rows: number };

// ========================================================================
// TerminalSession
// ========================================================================

/// One SpaceTerm session: a PTY-backed shell whose output drives both the webview's
/// xterm.js grid (raw bytes) and the `core` parser (block list for rich output).
export class TerminalSession {
  private constructor(
    private readonly panel: vscode.WebviewPanel,
    private readonly child: pty.IPty,
    private readonly core: SpaceTermTerminal,
  ) {}

  static create(context: vscode.ExtensionContext): TerminalSession {
    const panel = vscode.window.createWebviewPanel(
      VIEW_TYPE,
      "SpaceTerm",
      vscode.ViewColumn.Active,
      { enableScripts: true, retainContextWhenHidden: true },
    );

    const addon = loadAddon(context.asAbsolutePath(ADDON_RELATIVE));
    const core = new addon.Terminal();
    const child = pty.spawn(DEFAULT_SHELL, [], {
      name: "xterm-256color",
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
      cwd: process.env.HOME,
      env: cleanEnv(),
    });

    panel.webview.html = renderHtml(panel.webview, context.extensionUri);
    const session = new TerminalSession(panel, child, core);
    session.wire();
    return session;
  }

  private wire(): void {
    this.child.onData((data) => {
      this.core.feed(Buffer.from(data, "utf8"));
      void this.panel.webview.postMessage({ type: "data", data });
      void this.panel.webview.postMessage({
        type: "blocks",
        json: this.core.blocksJson(),
      });
    });

    this.panel.webview.onDidReceiveMessage((msg: HostToExtMessage) => {
      if (msg.type === "input") {
        this.child.write(msg.data);
      } else if (msg.type === "resize") {
        this.child.resize(msg.cols, msg.rows);
      }
    });

    this.panel.onDidDispose(() => this.child.kill());
  }
}

// ========================================================================
// Helpers
// ========================================================================

function cleanEnv(): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined) {
      env[key] = value;
    }
  }
  return env;
}

function renderHtml(webview: vscode.Webview, extensionUri: vscode.Uri): string {
  const scriptUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "dist", "webview.js"),
  );
  const styleUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "xterm.css"),
  );
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src ${webview.cspSource}`,
    `img-src ${webview.cspSource} data:`,
    "frame-src 'self'",
  ].join("; ");

  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <link rel="stylesheet" href="${styleUri}" />
  </head>
  <body>
    <div id="terminal"></div>
    <div id="blocks"></div>
    <script src="${scriptUri}"></script>
  </body>
</html>`;
}

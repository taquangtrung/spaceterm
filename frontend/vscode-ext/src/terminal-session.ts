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
// Coalesce block (rich-output) serialization: PTY bursts arrive in many small
// chunks, but blocksJson() is expensive and the webview only needs the latest.
const BLOCKS_FLUSH_MS = 16;

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
  private blocksTimer: ReturnType<typeof setTimeout> | undefined;

  private constructor(
    private readonly panel: vscode.WebviewPanel,
    private readonly child: pty.IPty,
    private readonly core: SpaceTermTerminal,
  ) {}

  /// Build a session, or surface an error and return `undefined` if the native
  /// addon can't be loaded or the shell can't be spawned.
  static create(context: vscode.ExtensionContext): TerminalSession | undefined {
    let core: SpaceTermTerminal;
    try {
      const addon = loadAddon(context.asAbsolutePath(ADDON_RELATIVE));
      core = new addon.Terminal();
    } catch (err) {
      void vscode.window.showErrorMessage(
        "SpaceTerm: failed to load the native addon. Build it with " +
          "`cargo build -p spaceterm-bindings` and copy it to " +
          `crates/bindings/spaceterm.node (${describeError(err)})`,
      );
      return undefined;
    }

    let child: pty.IPty;
    try {
      child = pty.spawn(DEFAULT_SHELL, [], {
        name: "xterm-256color",
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        cwd: process.env.HOME,
        env: cleanEnv(),
      });
    } catch (err) {
      void vscode.window.showErrorMessage(
        `SpaceTerm: failed to start shell "${DEFAULT_SHELL}" (${describeError(err)})`,
      );
      return undefined;
    }

    const panel = vscode.window.createWebviewPanel(
      VIEW_TYPE,
      "SpaceTerm",
      vscode.ViewColumn.Active,
      { enableScripts: true, retainContextWhenHidden: true },
    );

    panel.webview.html = renderHtml(panel.webview, context.extensionUri);
    const session = new TerminalSession(panel, child, core);
    session.wire();
    return session;
  }

  private wire(): void {
    this.child.onData((data) => {
      this.core.feed(Buffer.from(data, "utf8"));
      void this.panel.webview.postMessage({ type: "data", data });
      this.scheduleBlocksFlush();
    });

    this.child.onExit(({ exitCode, signal }) => {
      const reason = signal ? `signal ${signal}` : `code ${exitCode}`;
      void this.panel.webview.postMessage({
        type: "data",
        data: `\r\n\x1b[2m[process exited: ${reason}]\x1b[0m\r\n`,
      });
      this.flushBlocks();
    });

    this.panel.webview.onDidReceiveMessage((msg: HostToExtMessage) => {
      if (msg.type === "input") {
        this.child.write(msg.data);
      } else if (msg.type === "resize") {
        this.child.resize(msg.cols, msg.rows);
      }
    });

    this.panel.onDidDispose(() => {
      if (this.blocksTimer !== undefined) {
        clearTimeout(this.blocksTimer);
      }
      this.child.kill();
    });
  }

  /// Coalesce block updates onto a short timer so a burst of PTY chunks results
  /// in a single (latest-wins) serialization rather than one per chunk.
  private scheduleBlocksFlush(): void {
    if (this.blocksTimer !== undefined) {
      return;
    }
    this.blocksTimer = setTimeout(() => {
      this.blocksTimer = undefined;
      this.flushBlocks();
    }, BLOCKS_FLUSH_MS);
  }

  private flushBlocks(): void {
    void this.panel.webview.postMessage({
      type: "blocks",
      json: this.core.blocksJson(),
    });
  }
}

// ========================================================================
// Helpers
// ========================================================================

function describeError(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

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
  const customStyleUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "spaceterm.css"),
  );
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src ${webview.cspSource}`,
    `img-src ${webview.cspSource} data:`,
    "frame-src 'self'",
  ].join("; ");

  const config = vscode.workspace.getConfiguration("terminal.integrated");
  const fontFamily = config.get<string>("fontFamily") || "";
  const fontSize = config.get<number>("fontSize") || 14;
  const fontWeight = config.get<string | number>("fontWeight") || "normal";
  const fontWeightBold = config.get<string | number>("fontWeightBold") || "bold";

  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <link rel="stylesheet" href="${styleUri}" />
    <link rel="stylesheet" href="${customStyleUri}" />
  </head>
  <body style="--spaceterm-font-family: ${fontFamily}; --spaceterm-font-size: ${fontSize}px; --spaceterm-font-weight: ${fontWeight}; --spaceterm-font-weight-bold: ${fontWeightBold};">
    <div id="terminal-container">
      <div id="terminal"></div>
    </div>
    <div id="blocks-container">
      <div class="blocks-header">
        <span class="blocks-title">Rich Output</span>
        <span class="blocks-subtitle">SpaceTerm Blocks</span>
      </div>
      <div id="blocks"></div>
    </div>
    <script src="${scriptUri}"></script>
  </body>
</html>`;
}


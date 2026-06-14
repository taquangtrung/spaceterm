import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";

import { renderBundle } from "@spaceterm/block-renderer";
import type { CommandBlock } from "@spaceterm/block-renderer";
import { registerRichRenderers } from "@spaceterm/rich-renderers";

// ========================================================================
// Data Structures
// ========================================================================

// Messages the extension host sends to this webview.
type HostMessage =
  | { type: "data"; data: string }
  | { type: "blocks"; json: string };

interface VsCodeApi {
  postMessage(message: unknown): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

// ========================================================================
// Webview entry
// ========================================================================

const vscode = acquireVsCodeApi();

// ========================================================================
// Theme & Font Synchronization Helpers
// ========================================================================

function getCssVar(name: string, fallback: string): string {
  return window.getComputedStyle(document.body).getPropertyValue(name).trim() || fallback;
}

function getTerminalTheme() {
  const isDark = document.body.classList.contains("vscode-dark") || 
                 (document.body.classList.contains("vscode-high-contrast") && 
                  !document.body.classList.contains("vscode-light"));
  
  const defaultBg = isDark ? "#121214" : "#f8f9fa";
  const defaultFg = isDark ? "#e4e4e7" : "#1a1a1a";
  const defaultCursor = isDark ? "#a1a1aa" : "#71717a";
  const defaultSelection = isDark ? "rgba(255, 255, 255, 0.12)" : "rgba(0, 0, 0, 0.08)";

  return {
    background: getCssVar("--vscode-terminal-background", getCssVar("--vscode-editor-background", defaultBg)),
    foreground: getCssVar("--vscode-terminal-foreground", getCssVar("--vscode-editor-foreground", defaultFg)),
    cursor: getCssVar("--vscode-terminal-cursorForeground", getCssVar("--vscode-editor-foreground", defaultCursor)),
    cursorAccent: getCssVar("--vscode-terminal-cursorBackground", getCssVar("--vscode-editor-background", defaultBg)),
    selectionBackground: getCssVar("--vscode-terminal-selectionBackground", getCssVar("--vscode-editor-selectionBackground", defaultSelection)),
    
    // ANSI colors matching VSCode theme precisely
    black: getCssVar("--vscode-terminal-ansiBlack", "#000000"),
    red: getCssVar("--vscode-terminal-ansiRed", "#cd3131"),
    green: getCssVar("--vscode-terminal-ansiGreen", "#0dbc79"),
    yellow: getCssVar("--vscode-terminal-ansiYellow", "#e5e510"),
    blue: getCssVar("--vscode-terminal-ansiBlue", "#2472c8"),
    magenta: getCssVar("--vscode-terminal-ansiMagenta", "#bc3fbc"),
    cyan: getCssVar("--vscode-terminal-ansiCyan", "#11a8cd"),
    white: getCssVar("--vscode-terminal-ansiWhite", "#e5e5e5"),
    
    brightBlack: getCssVar("--vscode-terminal-ansiBrightBlack", "#666666"),
    brightRed: getCssVar("--vscode-terminal-ansiBrightRed", "#f14c4c"),
    brightGreen: getCssVar("--vscode-terminal-ansiBrightGreen", "#23d18b"),
    brightYellow: getCssVar("--vscode-terminal-ansiBrightYellow", "#f5f543"),
    brightBlue: getCssVar("--vscode-terminal-ansiBrightBlue", "#3b8eea"),
    brightMagenta: getCssVar("--vscode-terminal-ansiBrightMagenta", "#d670d6"),
    brightCyan: getCssVar("--vscode-terminal-ansiBrightCyan", "#29b8db"),
    brightWhite: getCssVar("--vscode-terminal-ansiBrightWhite", "#e5e5e5")
  };
}

function getTerminalFontFamily(): string {
  return getCssVar("--vscode-editor-font-family", "Consolas, 'Courier New', monospace");
}

function getTerminalFontSize(): number {
  const sizeStr = getCssVar("--vscode-editor-font-size", "13px");
  const size = parseInt(sizeStr);
  return isNaN(size) ? 13 : size;
}

function getTerminalFontWeight(): "normal" | "bold" | "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900" {
  const weight = getCssVar("--vscode-editor-font-weight", "normal");
  return weight as any;
}

// ========================================================================
// Mount UI
// ========================================================================

function mount(): void {
  registerRichRenderers();

  const terminalHost = document.getElementById("terminal");
  const blocksHost = document.getElementById("blocks");
  if (!terminalHost || !blocksHost) {
    return;
  }

  const term = new Terminal({
    convertEol: true,
    fontFamily: getTerminalFontFamily(),
    fontSize: getTerminalFontSize(),
    fontWeight: getTerminalFontWeight(),
    lineHeight: 1.45,
    theme: getTerminalTheme(),
    cursorBlink: true,
    cursorStyle: "bar",
    cursorWidth: 2,
    allowProposedApi: true
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(terminalHost);
  fitAddon.fit();

  term.onData((data) => vscode.postMessage({ type: "input", data }));

  term.onResize(({ cols, rows }) => {
    vscode.postMessage({ type: "resize", cols, rows });
  });

  const observer = new ResizeObserver(() => fitAddon.fit());
  observer.observe(terminalHost);

  // Synchronize terminal theme dynamically with VSCode theme/font changes
  const themeObserver = new MutationObserver(() => {
    term.options.theme = getTerminalTheme();
    term.options.fontFamily = getTerminalFontFamily();
    term.options.fontSize = getTerminalFontSize();
    term.options.fontWeight = getTerminalFontWeight();
    fitAddon.fit();
  });
  themeObserver.observe(document.body, {
    attributes: true,
    attributeFilter: ["class", "style"]
  });

  window.addEventListener("message", (event: MessageEvent<HostMessage>) => {
    const message = event.data;
    if (message.type === "data") {
      term.write(message.data);
    } else {
      renderContentBlocks(blocksHost, message.json);
    }
  });
}

/// Render only the rich (content) segments into the blocks panel; xterm.js owns
/// the text grid. The JSON comes from our own host over a trusted channel, so it
/// is treated as `CommandBlock[]` without further validation.
let lastBlocksJson = "";

function renderContentBlocks(host: HTMLElement, json: string): void {
  if (json === lastBlocksJson) {
    return;
  }
  lastBlocksJson = json;
  const blocks = JSON.parse(json) as CommandBlock[];
  host.replaceChildren();
  for (const block of blocks) {
    for (const segment of block.output) {
      if (segment.kind === "content") {
        host.appendChild(renderBundle(segment.data));
      }
    }
  }
}

mount();

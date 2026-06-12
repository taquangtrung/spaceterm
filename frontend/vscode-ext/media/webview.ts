import { Terminal } from "@xterm/xterm";

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

function mount(): void {
  registerRichRenderers();

  const terminalHost = document.getElementById("terminal");
  const blocksHost = document.getElementById("blocks");
  if (!terminalHost || !blocksHost) {
    return;
  }

  const term = new Terminal({ convertEol: true, fontFamily: "monospace" });
  term.open(terminalHost);
  term.onData((data) => vscode.postMessage({ type: "input", data }));

  term.onResize(({ cols, rows }) => {
    vscode.postMessage({ type: "resize", cols, rows });
  });

  const observer = new ResizeObserver(() => term.fit?.());
  observer.observe(terminalHost);

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

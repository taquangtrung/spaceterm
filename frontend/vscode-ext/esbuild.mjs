import { copyFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import esbuild from "esbuild";

const dir = path.dirname(fileURLToPath(import.meta.url));

// Resolve workspace TS packages straight from source (no separate build step).
const alias = {
  "@spaceterm/block-renderer": path.join(dir, "../block-renderer/src/index.ts"),
  "@spaceterm/palette": path.join(dir, "../palette/src/index.ts"),
  "@spaceterm/rich-renderers": path.join(dir, "../rich-renderers/src/index.ts"),
};

// Extension host bundle (Node/CommonJS). vscode, node-pty, and the native addon
// are resolved at runtime, not bundled.
await esbuild.build({
  entryPoints: [path.join(dir, "src/extension.ts")],
  outfile: path.join(dir, "dist/extension.js"),
  bundle: true,
  platform: "node",
  format: "cjs",
  external: ["vscode", "node-pty", "*.node"],
  alias,
});

// Webview bundle (browser/IIFE): xterm.js + the shared block-renderer.
await esbuild.build({
  entryPoints: [path.join(dir, "media/webview.ts")],
  outfile: path.join(dir, "dist/webview.js"),
  bundle: true,
  platform: "browser",
  format: "iife",
  alias,
});

copyFileSync(
  path.join(dir, "node_modules/@xterm/xterm/css/xterm.css"),
  path.join(dir, "media/xterm.css"),
);

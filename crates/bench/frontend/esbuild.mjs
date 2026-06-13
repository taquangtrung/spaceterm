import { copyFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import esbuild from "esbuild";

const dir = path.dirname(fileURLToPath(import.meta.url));
const dist = path.join(dir, "dist");
mkdirSync(dist, { recursive: true });

// Webview harness bundle (browser/IIFE): xterm.js + the WebGL addon + timing.
await esbuild.build({
  entryPoints: [path.join(dir, "harness.ts")],
  outfile: path.join(dist, "harness.js"),
  bundle: true,
  platform: "browser",
  format: "iife",
});

// xterm core needs its stylesheet for cell layout; the Rust host inlines it.
copyFileSync(
  path.join(dir, "node_modules/@xterm/xterm/css/xterm.css"),
  path.join(dist, "xterm.css"),
);

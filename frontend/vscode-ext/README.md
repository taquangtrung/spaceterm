# SpaceTerm (VSCode extension)

A rich, web-native terminal inside VSCode. The extension host spawns a PTY
(`node-pty`), feeds its bytes to both xterm.js (the text grid, in the webview) and
the `spaceterm-core` parser (via the `spaceterm-bindings` napi addon), and renders emitted
[TBP](../../docs/terminal-block-protocol-spec.md) content blocks with the shared
[`@spaceterm/block-renderer`](../block-renderer).

## Architecture

```
PTY bytes ──┬─► spaceterm-bindings (core)  ─► blocksJson ─► webview ─► block-renderer ─► #blocks
            └─► webview ─► xterm.js ─► #terminal
keystrokes ◄─ webview (xterm.onData) ─► host ─► pty.write
```

- `src/extension.ts` — activation; registers `spaceterm.open` and `spaceterm.commandPalette`
  (the palette uses `@spaceterm/palette`'s `VSCodePalette` → host QuickPick).
- `src/terminal-session.ts` — PTY ⇄ core ⇄ webview wiring.
- `media/webview.ts` — xterm.js grid + block-renderer for content blocks.

## Build

Prerequisites: build the native addon first and copy it where the host expects it:

```bash
cargo build -p spaceterm-bindings
cp ../../target/debug/libspaceterm_bindings.so ../../crates/bindings/spaceterm.node
```

Then, in this directory:

```bash
pnpm install                 # set allowBuilds esbuild/node-pty to true to run the build
pnpm run typecheck           # tsc (host + webview), no host required
pnpm run build               # esbuild → dist/extension.js + dist/webview.js
```

Press F5 in VSCode (Extension Development Host) to run `SpaceTerm: Open Terminal`.

> Status: compile-verified. Running requires a VSCode host. xterm.js renders text
> faithfully; rich blocks render in the `#blocks` panel. Inline block placement
> within the scrollback is a later refinement.

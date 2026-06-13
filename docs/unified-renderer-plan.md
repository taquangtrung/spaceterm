# Unified renderer plan

> **Status: proposal.** Sequences the migration to a single web-first renderer
> shared by the native app and the VSCode extension, so both produce identical
> visual content. Phase 0 is a gate: its result decides whether we unify the
> text grid too, or only the rich blocks.

## Goal

Today SpaceTerm has **two independent renderers** that already diverge:

| Concern | Native (`crates/spaceterm`, `crates/render`) | VSCode (`frontend/vscode-ext`) |
|---|---|---|
| Text grid | `wgpu` + `glyphon`, full `Theme`, configurable font | `xterm.js`, hardcoded `monospace`, no theme |
| Rich blocks | Rust HTML generation in `webview.rs`, `wry` child webviews | TS `block-renderer` DOM, Electron webview |
| Block layout | inline in the grid (8 rows per block) | separate `#blocks` panel |
| MIME richness order | `webview.rs:22` | `block-renderer.ts:18` (already out of sync) |
| Markdown | hand-rolled regex | `marked` v12 |
| Math (KaTeX) | unsupported | supported |

The parsing core (`spaceterm-core`, `spaceterm-proto`) is shared, so the *data*
is identical; only pixel production differs. The fix is one renderer for the
whole UI.

## Decision: web-first, one bundle

A VSCode extension can only draw inside a webview, so the common GPU surface is
the web stack (WebGL2 floor, WebGPU where available). The native app already
hosts a webview (`wry`), so it can run the same frontend the extension runs.

**Target architecture:** one frontend bundle renders both text and blocks:

- **Text grid:** `xterm.js` + `@xterm/addon-webgl` (GPU glyph atlas; the engine
  VSCode itself ships).
- **Rich blocks:** the existing `frontend/block-renderer` + `rich-renderers`,
  laid out **inline** in the same scroll surface as the grid.
- **One theme object** (colors, font family, font size) fed to both layers.

Both frontends load this identical bundle:
- VSCode: in its webview (already does, via `media/webview.ts`).
- Native: in a full-window `wry` webview, replacing the `wgpu` grid +
  `wry`-blocks split. The native binary becomes a thin host (PTY + window +
  config + message bridge).

The Rust block-HTML generators (`render_block_html`, `markdown_to_html`,
`csv_to_table` in `crates/spaceterm/src/terminal/webview.rs`) are deleted; the
TS renderer becomes the single source of truth.

### Known residual gap

VSCode webview is Chromium; `wry` is WebKitGTK (Linux) / WKWebView (macOS) /
WebView2-Chromium (Windows). The grid renders to `<canvas>` (pixel-deterministic
across engines), so it matches. Rich blocks are DOM/CSS and can differ subtly
between WebKit and Chromium. We accept this for best-effort rich content; exact
block parity would require shipping a Chromium runtime in the native shell.

## Phase 0 — perf spike (GATE)

The native text path is currently `wgpu`-direct-to-window, the "native-class
speed" selling point. Before committing to route it through a webview, measure.

**Build:** implemented in `crates/bench` (see its README). It
feeds one deterministic VT corpus through both a `wry` window loading
`xterm.js` + `@xterm/addon-webgl` (`webgl` mode) and the current `wgpu` +
`glyphon` renderer (`glyphon` mode), and prints throughput + frame-time
percentiles for each.

**Measure:** sustained throughput (bytes/s without dropped frames), p99 frame
time, input-to-paint latency, idle/scroll smoothness.

**Run:** on a real machine with a display + GPU (this is a local measurement;
it cannot be run headless in CI).

**Pass bar:** `xterm.js`-webgl in `wry` stays within a chosen factor (suggest
1.5x) of the current renderer on sustained throughput and p99 frame time.

**Outcome:**
- **Pass** -> proceed to full unification (Phases 1-4).
- **Fail** -> fall back: VSCode uses the web bundle, native keeps `wgpu`-direct
  for the grid, and we unify **blocks only** (still deletes the duplicated Rust
  block rendering and fixes the largest divergence). Skip Phases 2-3's grid
  parts; keep Phase 1 (theme) and Phase 4 (tests).

## Phase 1 — shared theme + GPU grid in the web layer

1. Define a serializable `RenderTheme` (colors as hex, font family, font size)
   derived from `crates/render/src/theme.rs::Theme` + config. Send it to the
   webview alongside the data stream.
2. In `media/webview.ts`, replace `new Terminal({ fontFamily: "monospace" })`
   with a `Terminal` configured from `RenderTheme` (`theme`, `fontFamily`,
   `fontSize`) and load `@xterm/addon-webgl` (fallback to canvas if WebGL2 is
   unavailable). Add the addon to `esbuild.mjs`'s webview bundle.
3. Map ANSI 0-15 from `Theme.ansi`, default fg/bg, cursor, selection into the
   `xterm.js` theme so colors match the native palette.

## Phase 2 — unified inline layout

1. Replace the separate `#terminal` / `#blocks` split in `media/webview.ts`
   with one scroll surface where content blocks are positioned **inline** at
   their grid row (mirroring the native 8-row inline model), so layout matches
   across frontends.
2. Move this surface into the shared frontend (a new `frontend/terminal-ui`
   package, or promote `media/webview.ts`) so the native shell loads the exact
   same entry point. `esbuild` aliases already resolve workspace packages from
   source (`esbuild.mjs`), so the bundle stays one artifact.

## Phase 3 — native shell hosts the bundle

1. Add a message bridge in the native app: PTY bytes -> webview (`data`),
   `Scrollback::to_json()` -> webview (`blocks`), webview input/resize -> PTY.
   This mirrors `frontend/vscode-ext/src/terminal-session.ts:74-93`.
2. Replace the `wgpu` grid render path (`crates/spaceterm/src/app/render.rs`,
   `crates/render`'s window renderer) and the `wry`-per-block tiles
   (`terminal/webview.rs`, `WebViewManager`) with a single full-window `wry`
   webview loading the shared bundle.
3. Delete `render_block_html`, `markdown_to_html`, `csv_to_table`, the
   `MIME_RICHNESS` array, and `block_shell.html`. Keep `crates/render` only if
   still wanted for the headless `make demo`/`dump_session` path; otherwise
   retire it.
4. Keep trust-tier enforcement: the TS `sandbox.ts` iframe model already covers
   isolated/restricted/trusted, so the Rust CSP injection is no longer needed.

## Phase 4 — parity tests

1. Add a fixture set of TBP bundles (multi-MIME, markdown, latex, svg, images,
   csv/json, trust tiers, plain-text fallback).
2. Assert both the shared richness order and renderer pick the same MIME per
   fixture and emit equivalent markup (snapshot). One renderer makes this a
   single snapshot suite instead of two that can drift.
3. Wire into `make test` so the renderers cannot silently diverge again.

## Sequencing summary

```
Phase 0 (spike, GATE) ─┬─ pass ─> Phase 1 ─> Phase 2 ─> Phase 3 ─> Phase 4
                       └─ fail ─> Phase 1 + Phase 4 (blocks-only unification)
```

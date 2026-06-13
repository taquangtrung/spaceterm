# spaceterm-bench

Phase 0 gate for [`docs/unified-renderer-plan.md`](../../docs/unified-renderer-plan.md).

It answers one question: **does `xterm.js` + `@xterm/addon-webgl` (in a `wry`
webview) keep up with the native `wgpu` + `glyphon` renderer?** If yes, the
unified renderer can run the same web bundle in both frontends. If no, we keep
the native `wgpu` grid and unify only the rich blocks.

Both backends feed one deterministic VT corpus (`src/corpus.rs`) and print
throughput plus frame-time percentiles.

## Requirements

A real display and GPU. Both modes open a window; neither runs headless.

## Run

```bash
# Build the webview harness bundle once (needs Node + pnpm):
cd crates/bench/frontend
pnpm install && pnpm build
cd -

# Baseline: existing wgpu + glyphon renderer.
cargo run -p spaceterm-bench -- glyphon

# Candidate: xterm.js + WebGL addon in a wry webview.
cargo run -p spaceterm-bench -- webgl

# Bigger corpus (default is 20000 lines):
cargo run -p spaceterm-bench -- glyphon --lines 100000
cargo run -p spaceterm-bench -- webgl --lines 100000
```

The `webgl` run prints which backend the page actually used (`webgl`, or
`canvas` if the WebGL addon failed to load in that webview engine) — note this,
since the native `wry` engine (WebKitGTK / WKWebView / WebView2) is not always
the same as VSCode's Chromium.

## Reading the result

Compare the two `throughput` and `frame p99` lines. The plan's suggested pass
bar: `webgl` stays within ~1.5x of `glyphon` on both. Run each mode a few times
and take the best; first runs pay font-scan and shader-warmup costs.

## Methodology / caveats

- Same corpus, same `CHUNK_BYTES`, so both render the same number of frames; the
  comparison is wall-clock over identical work.
- Both present under vsync (the native renderer uses `AutoVsync`), so sustained
  throughput is frame-bound, not raw-parse-bound. That is the realistic metric
  for interactive use; it is not a measure of headless parse speed.
- This is a spike: blunt timing, `expect()` on setup, no retries. It exists to
  produce a number, not to ship.

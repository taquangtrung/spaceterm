# SpaceTerm

> **Early development.** SpaceTerm is not production-ready. APIs, wire formats, and
> config schema may change without notice. Expect rough edges and missing features.

A web-native terminal: native-class text speed, with output modeled as a sequence
of typed, MIME-tagged **blocks** that a real web engine can render inline (tables,
charts, math, PDFs, images) — all with a `text/plain` fallback everywhere else.

See [`docs/usage-guide.md`](docs/usage-guide.md) for how to use SpaceTerm, and
[`docs/terminal-block-protocol-spec.md`](docs/terminal-block-protocol-spec.md) for the
protocol (TBP).

## Prerequisites

- **Rust** (stable ≥ 1.80; a recent nightly works too) — `cargo`
- **Node** ≥ 18 and **pnpm** (for the frontend packages / extension)
- **uv** (for the Python client tests) — optional
- A C toolchain (for `node-pty` if you actually run the extension)

## Quick start

```bash
make build        # build all Rust crates
make rust-test    # 55 Rust tests
make test         # everything: Rust + frontend (TS) + Python
make lint         # clippy (deny warnings) + rustfmt check
make help         # all targets
```

## Try it

### 1. The integrated native pipeline (headless, no display needed)

The `spaceterm` binary runs a command under a PTY and prints **both** views of its
output: the live screen grid (`render` crate) and the parsed block list (`core`).

```bash
make demo CMD='ls -la'
# or directly:
cargo run -p spaceterm -- bash -c 'echo hi; printf "\033[1;32mgreen\033[0m\n"'
```

### 2. Emit a rich block from a tool, watch the core parse it

The `dump_session` example runs a command under a PTY and prints the parsed
`CommandBlock` list. Let the command itself emit a TBP block (so it rides the PTY
the example reads), with `SPACETERM=1` to enable emission:

```bash
# Shell client -> SVG block:
printf '<svg width=10/>' > /tmp/plot.svg
cargo run -p spaceterm-core --example dump_session -- \
  bash -c "SPACETERM=1 $PWD/clients/client.sh svg /tmp/plot.svg"

# Python client -> SVG block:
cargo run -p spaceterm-core --example dump_session -- \
  bash -c "SPACETERM=1 PYTHONPATH=$PWD/clients/client-py/src python3 -c \
    'import spaceterm; spaceterm.display_svg(\"<svg width=10/>\", text=\"fallback\")'"
```

The emitted SVG appears as a `Content` block with its MIME bundle and a
`text/plain` fallback. (Without `SPACETERM=1`, the clients print the plain-text
fallback instead — the safe degradation path.)

### 3. The VSCode extension (needs VSCode)

```bash
make vscode        # builds the napi addon + typechecks the extension
cd frontend/vscode-ext
# set allowBuilds esbuild/node-pty to true in pnpm-workspace.yaml, then:
pnpm install && pnpm run build
```

Open the folder in VSCode and press **F5** (Extension Development Host), then run
**“spaceterm: Open Terminal”**. Text renders in xterm.js; emitted blocks (HTML, KaTeX,
Markdown, images, SVG) render in the blocks panel. See
[`frontend/vscode-ext/README.md`](frontend/vscode-ext/README.md).

### 4. Frontend package tests

```bash
make frontend-test        # block-renderer, palette, rich-renderers + ext typecheck
# or individually:
cd frontend/block-renderer && pnpm install && pnpm test
```

## License

MIT OR Apache-2.0.

# spaceterm

The native app crate for [SpaceTerm](../../README.md) — a web-native terminal emulator.

> **Early development.** Not production-ready; APIs and behaviour may change without notice.

## What this crate is

`spaceterm` is the top-level binary and app library. It wires together all the
other crates into a runnable terminal:

- **`spaceterm` binary** — the entry point. Runs in window mode or headless demo mode.
- **`spaceterm_app` library** — the public API surface: `App`, layout primitives,
  input/action types, interaction modes, pane management, and KDL config parsing.

The heavy lifting lives in the supporting crates:

| Crate | Role |
|---|---|
| [`spaceterm-proto`](../proto) | TBP wire types and OSC codec |
| [`spaceterm-core`](../core) | PTY driver, vte parser, block-list scrollback |
| [`spaceterm-render`](../render) | VT cell grid (CPU side of the GPU text renderer) |
| [`spaceterm-mux`](../mux) | Headless mux server, attach/detach, SSH remote |

## Install

```bash
cargo install spaceterm
```

Requires Rust >= 1.80 and a C toolchain. On Linux, `libgtk-3-dev` and
`libwebkit2gtk-4.1-dev` are needed for WebView tile support (rich block rendering).

## Usage

```bash
# Open a GPU-accelerated terminal window:
spaceterm

# Run a command headlessly and print the parsed block list:
spaceterm bash -c 'echo hello'

# Mux subcommands:
spaceterm mux serve
spaceterm mux attach default
```

See the [usage guide](../../docs/usage-guide.md) for configuration, keybindings,
rich block rendering, and client libraries.

## License

MIT OR Apache-2.0

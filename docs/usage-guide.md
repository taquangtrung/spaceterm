# SpaceTerm Usage Guide

SpaceTerm is a web-native terminal emulator. It runs your shell in a PTY and
parses the output into typed, MIME-tagged **blocks** — plain text renders on a
GPU grid at native speed, while rich content (HTML, SVG, Markdown, LaTeX,
images) renders inline via WebView tiles.

## Table of Contents

- [Building](#building)
- [Running](#running)
  - [Window Mode](#window-mode)
  - [Headless Demo](#headless-demo)
- [Configuration](#configuration)
- [Interaction Modes](#interaction-modes)
  - [Insert Mode](#insert-mode)
  - [Normal Mode](#normal-mode)
  - [Block-Focus Mode](#block-focus-mode)
- [Keybinding Reference](#keybinding-reference)
- [Rich Block Rendering](#rich-block-rendering)
- [Client Libraries](#client-libraries)
  - [Shell Client](#shell-client)
  - [Python Client](#python-client)
- [Environment Variables](#environment-variables)
- [VSCode Extension](#vscode-extension)
- [Multiplexer](#multiplexer)
- [Testing](#testing)

## Building

Requirements: Rust >= 1.80, Node >= 18 + pnpm (for frontend), a C toolchain.

```bash
make build          # build all Rust crates
make test           # run all tests (Rust + frontend + Python)
make lint           # clippy (deny warnings) + rustfmt check
```

## Running

### Window Mode

Launch with no arguments to open a GPU-accelerated terminal window:

```bash
cargo run -p spaceterm
```

This opens a winit + wgpu window with an interactive shell. Rich blocks emitted
by tools running inside the terminal are rendered inline as WebView tiles.

### Headless Demo

Pass a command to run it under a PTY and print the parsed block list and screen
grid:

```bash
cargo run -p spaceterm -- bash -c 'echo hello'
```

Or use the Makefile target:

```bash
make demo CMD='ls -la'
```

## Configuration

SpaceTerm reads a KDL config file from:

- `$XDG_CONFIG_HOME/spaceterm/spaceterm.kdl`, or
- `~/.config/spaceterm/spaceterm.kdl`

If no config file exists, sensible defaults are used (dark theme, 15pt system
font, full opacity).

### Example Config

```kdl
theme "auto"
font "FiraCode Nerd Font"
font-size "15"
opacity "1.0"

colors {
    background "#2a2f31"
    foreground "#d8d8d8"
    cursor-bg "#52ad70"
    selection-bg "#fffacd"
    split "#51554f"
    visual-bell "#202020"
    ansi "#000000" "#c22727" "#71b312" "#faa213" "#4fa2fa" "#bb67b2" "#21afbf" "#c0c0c0"
    brights "#7a7a7a" "#d43f30" "#71b312" "#ebb909" "#5da2eb" "#c97df5" "#04cfe1" "#e1ebfa"
    indexed 136 "#af8700"
}

keybindings {
    normal {
        binding "j" "focus_down"
        binding "k" "focus_up"
    }
    insert {
        binding "Ctrl-Space" "toggle_mode"
    }
}
```

### Config Options

| Option | Values | Default |
|--------|--------|---------|
| `theme` | `"dark"`, `"light"`, `"auto"` | `"dark"` |
| `font` | Any font family name | System font |
| `font-size` | Float as string | `"15"` |
| `opacity` | `0.1`–`1.0` | `"1.0"` |
| `colors` | Block with hex color overrides | Built-in dark/light presets |
| `keybindings` | Per-mode binding blocks | Built-in vi-style defaults |

### Color Overrides

Inside the `colors` block, all fields are optional — unset colors keep the
active theme preset. Available fields:

- `background`, `foreground` — main colors
- `cursor-bg`, `cursor-fg` — cursor colors
- `selection-bg`, `selection-fg` — text selection colors
- `split` — divider between panes
- `visual-bell` — bell flash color
- `ansi` — 8 standard colors (space-separated)
- `brights` — 8 bright colors (space-separated)
- `indexed` — indexed 256-color entries (`indexed <slot> "#hex"`)

## Interaction Modes

SpaceTerm uses a **modal interaction model** with three modes per pane, similar
to vim. This keeps the keyboard-driven workflow fast while allowing rich block
navigation.

### Insert Mode

The default. Keys are sent directly to the shell running in the PTY — this is
how a normal terminal works. `Esc` or `Ctrl-Shift-Space` switches to Normal
mode.

When a fullscreen application (vim, less, htop) is using the alternate screen,
`Esc` stays bound to the application instead of switching modes.

### Normal Mode

SpaceTerm intercepts all keys for block navigation, cursor traversal, and pane
management. Keys never reach the PTY in this mode.

Use Normal mode to:

- Navigate the block list (each command's output is a block)
- Scroll through history with vim-style motions
- Split, close, and focus panes
- Search across blocks
- Yank (copy) block content
- Fold/unfold blocks
- Use quick-select to jump to blocks

Press `i` or `Esc` to return to Insert mode.

### Block-Focus Mode

When a rich block (HTML, SVG, etc.) is focused, keys forward to the block's
WebView for interactive content. Press `Esc` to return to Normal mode.

## Keybinding Reference

### Insert Mode

| Key | Action |
|-----|--------|
| `Esc` | Switch to Normal mode (unless fullscreen app is active) |
| `Ctrl-Shift-Space` | Switch to Normal mode (always works) |
| `Ctrl-Shift-C` | Copy selection to clipboard |
| `Ctrl-Shift-V` | Paste from clipboard |
| `Ctrl-Shift-P` | Toggle command palette |
| All other keys | Sent to the PTY |

### Normal Mode — Cursor Motion

| Key | Action |
|-----|--------|
| `h` / `Left` | Move cursor left |
| `j` / `Down` | Move cursor down |
| `k` / `Up` | Move cursor up |
| `l` / `Right` | Move cursor right |
| `0` | Move to start of line |
| `$` | Move to end of line |
| `^` | Move to first non-blank character |
| `w` | Word forward |
| `b` | Word back |
| `e` | Word end |
| `W` | WORD forward (punctuation included) |
| `B` | WORD back |
| `E` | WORD end |
| `gg` | Jump to top of buffer |
| `G` | Jump to bottom of buffer |
| `Ctrl-d` | Half page down |
| `Ctrl-u` | Half page up |
| `PageDown` | Full page down |
| `PageUp` | Full page up |

### Normal Mode — Panes

| Key | Action |
|-----|--------|
| `v` | Split pane vertically |
| `s` | Split pane horizontally |
| `x` | Close current pane |
| `Ctrl-h` | Focus pane to the left |
| `Ctrl-j` | Focus pane down |
| `Ctrl-k` | Focus pane up |
| `Ctrl-l` | Focus pane to the right |

### Normal Mode — Blocks

| Key | Action |
|-----|--------|
| `]b` | Focus next block |
| `[b` | Focus previous block |
| `za` | Toggle fold on current block |
| `y` | Yank (copy) current block's text |
| `q` | Enter quick-select mode (jump to a visible block) |
| `Enter` | Focus current block (enter Block-Focus mode) |

### Normal Mode — Search

| Key | Action |
|-----|--------|
| `/` | Start search |
| (in search) type | Append to search query |
| (in search) `Backspace` | Delete last character |
| (in search) `Enter` | Execute search (jump to next match) |
| (in search) `Esc` | Cancel search |
| `n` | Next search match |
| `N` | Previous search match |

### Normal Mode — Mode Switching

| Key | Action |
|-----|--------|
| `i` | Return to Insert mode |
| `Esc` | Return to Insert mode |

### Block-Focus Mode

| Key | Action |
|-----|--------|
| `Esc` | Return to Normal mode |
| All other keys | Forwarded to the block's WebView |

### Global (All Modes)

| Key | Action |
|-----|--------|
| Mouse click | Focus pane at click position |
| Double click | Select word |
| Click + drag | Extend selection |
| Scroll wheel | Scroll pane history |
| Middle click | Paste from clipboard |

## Rich Block Rendering

SpaceTerm uses the **Terminal Block Protocol** (TBP) over OSC 9001 to receive
rich content. Any tool that emits a TBP block to its stdout will have that
content rendered inline — no plugins or extensions needed.

Supported MIME types:

| MIME | Render |
|------|--------|
| `text/plain` | Always rendered as terminal text (the fallback) |
| `text/html` | WebView tile |
| `image/svg+xml` | WebView tile |
| `text/markdown` | Rendered to HTML, displayed as WebView tile |
| `text/latex` | Rendered via KaTeX, displayed as WebView tile |
| `image/png`, `image/jpeg`, `image/gif`, `image/webp` | Base64-encoded in WebView tile |

Every block carries a `text/plain` fallback. When SpaceTerm is not the active
terminal (e.g., piping output, running under tmux/ssh/CI), the fallback is
printed instead — tools degrade gracefully without any configuration.

See [`terminal-block-protocol-spec.md`](terminal-block-protocol-spec.md) for the
full protocol specification.

## Client Libraries

### Shell Client

`clients/client.sh` provides shell functions and a CLI for emitting TBP blocks.

**As a CLI:**

```bash
# Emit an SVG file:
SPACETERM=1 ./clients/client.sh svg /tmp/plot.svg

# Emit an HTML fragment:
SPACETERM=1 ./clients/client.sh html /tmp/report.html

# Emit Markdown (from stdin):
echo "# Hello" | SPACETERM=1 ./clients/client.sh markdown -

# Emit a raster image:
SPACETERM=1 ./clients/client.sh image chart.png

# Check if SpaceTerm is the active terminal:
./clients/client.sh supported
```

**As a library (source it):**

```bash
source clients/client.sh

spaceterm_emit_svg /tmp/plot.svg
spaceterm_emit_html "<h1>Hello</h1>"
spaceterm_emit_markdown "# Hello"
spaceterm_emit_latex "E = mc^2"
spaceterm_emit_image photo.png
```

The `SPACETERM=1` environment variable (or `TERM_PROGRAM=spaceterm`) enables
emission. Without it, the plain-text fallback is printed instead.

### Python Client

`clients/client-py/` provides a Python package for emitting TBP blocks.

```bash
pip install -e clients/client-py
```

**Usage:**

```python
import spaceterm

spaceterm.display(dataframe)                  # uses _repr_*_ methods
spaceterm.display_html("<h1>Hello</h1>")
spaceterm.display_svg(open("plot.svg").read())
spaceterm.display_markdown("# Hello")
spaceterm.display_latex(r"E = mc^2")
spaceterm.display_image("chart.png")
```

All `display_*` functions accept optional keyword arguments:

- `text` — override the plain-text fallback
- `title` — block title
- `height_hint` — suggested height in pixels
- `trust` — trust tier (`"restricted"`, `"isolated"`, or `"trusted"`)

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `SPACETERM` | Set to `1` to enable TBP emission in clients |
| `SPACETERM_SHELL` | Override the shell binary (default: `$SHELL`, then `/bin/bash`) |
| `SPACETERM_FONT` | Override the font family (takes priority over config file) |
| `SPACETERM_FONT_SIZE` | Override the font size (takes priority over config file) |
| `SPACETERM_GPU_DEBUG` | Set to `1` to enable wgpu validation layers (for renderer debugging) |
| `SPACETERM_SIDECHANNEL_DIR` | Directory for TBP side-channel files (set internally) |

## VSCode Extension

SpaceTerm ships a VSCode extension that embeds the terminal in a VSCode panel,
rendering text via xterm.js and rich blocks in a WebView.

```bash
make vscode        # build the napi addon + typecheck the extension
cd frontend/vscode-ext
pnpm install && pnpm run build
```

Open the project in VSCode, press **F5** to launch Extension Development Host,
then run **"SpaceTerm: Open Terminal"** from the command palette.

Commands:

- `spaceterm: Open Terminal` — open a new SpaceTerm terminal panel
- `spaceterm: Command Palette` — open the built-in command palette

See [`frontend/vscode-ext/README.md`](../frontend/vscode-ext/README.md) for
details.

## Multiplexer

SpaceTerm includes a multiplexer for headless PTY sessions with attach/detach
support over Unix sockets.

```bash
# Start the mux server:
spaceterm mux serve

# List sessions:
spaceterm mux list

# Attach to a session (default: "default"):
spaceterm mux attach
spaceterm attach my-session

# Kill a session:
spaceterm mux kill my-session
```

The mux server socket lives at `$XDG_RUNTIME_DIR/spaceterm-mux.sock`
(or `/tmp/spaceterm-mux.sock` as fallback).

## Testing

| Component | Command |
|-----------|---------|
| All Rust crates | `make rust-test` |
| Frontend (TS) | `make frontend-test` |
| Python client | `make py-test` |
| Everything | `make test` |
| Lint | `make lint` |
| Format | `make fmt` |
| Headless demo | `make demo CMD='ls -la'` |

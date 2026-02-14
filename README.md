# Termania

A GPU-accelerated multi-pane terminal emulator for macOS. Termania renders a grid of independent terminal sessions in a single window using wgpu, designed for monitoring multiple agents, builds, or services at a glance.

## Features

- **Grid layout** -- configurable NxN grid of terminal panes (default 3x3)
- **GPU-accelerated rendering** via wgpu/Metal
- **Session configuration** -- separate `sessions.toml` defines what runs in each pane
- **Broadcast mode** -- type into all panes simultaneously
- **Multi-select** -- select panes with Shift+drag or Cmd+click, then send commands to the group
- **Command overlay** -- quick command bar to dispatch a command to all/selected panes
- **Error detection** -- panes tint red when error output is detected
- **Inactive pane fading** -- unfocused panes dim automatically
- **Watermark text** -- large faded text behind terminal content for pane identification
- **Initial commands** -- auto-run commands after shell startup
- **Text Tap API** -- Unix socket server for external processes to read terminal content or send input
- **Mouse click focus** -- click a pane to focus it
- **Full ANSI/VTE support** -- 256-color, truecolor, alternate screen, scroll regions

## Installation

```bash
cargo build --release
# Optionally symlink to PATH:
sudo ln -sf $(pwd)/target/release/termania /usr/local/bin/termania
```

## Quick Start

```bash
# Run with defaults (3x3 grid of shells)
termania

# Run with debug logging
RUST_LOG=debug termania
```

## Configuration

Termania uses two config files:

| File | Purpose |
|------|---------|
| `config.toml` | Visual settings: font, colors, grid layout, window size |
| `sessions.toml` | Session settings: what command runs in each pane, cwd, watermarks |

### Config File Locations

**config.toml** is loaded from:
- `~/Library/Application Support/termania/config.toml` (macOS)

If no config exists, Termania writes a default one on first launch.

**sessions.toml** is loaded in this order (first found wins):
1. `./sessions.toml` (current working directory -- per-project)
2. `~/Library/Application Support/termania/sessions.toml` (global default)
3. Falls back to `[[panes]]` in `config.toml` if no sessions file exists

### config.toml

```toml
[font]
family = "SF Mono"
size = 14.0
line_height = 1.2
letter_spacing = 0.0

[grid]
rows = 3
cols = 3
gap = 4               # pixels between panes
inner_padding = 4      # padding inside each pane
outer_padding = 4      # padding around the entire grid
title_bar_height = 24

[window]
width = 1920
height = 1080
title = "Termania"

[colors]
background = "#010409"
foreground = "#e6edf3"
cursor = "#f0f6fc"
selection = "#264f78"
border = "#30363d"
border_focused = "#58a6ff"
title_bg = "#0d1117"
title_fg = "#e6edf3"

# ANSI palette (16 colors: 8 normal + 8 bright)
ansi = [
    "#0d1117", "#ff7b72", "#3fb950", "#d29922",
    "#58a6ff", "#bc8cff", "#39d353", "#c9d1d9",
    "#484f58", "#ffa198", "#56d364", "#e3b341",
    "#79c0ff", "#d2a8ff", "#56d364", "#f0f6fc",
]

[text_tap]
enabled = true
socket_path = "/tmp/termania.sock"
```

### sessions.toml

```toml
title = "My Project"       # overrides window title from config.toml
rows = 2                   # overrides grid rows
cols = 3                   # overrides grid cols

[[panes]]
title = "API Server"
cwd = "~/projects/api"
initial_commands = ["npm run dev"]
watermark = "API"

[[panes]]
title = "Frontend"
cwd = "~/projects/web"
command = "/bin/zsh"       # override shell (default: $SHELL)
initial_commands = ["npm start"]
watermark = "WEB"

[[panes]]
title = "Tests"
cwd = "~/projects/api"
initial_commands = ["npm test -- --watch"]

[[panes]]
title = "Logs"
cwd = "/var/log"

[[panes]]
title = "Agent 1"
cwd = "~/projects/agent"
initial_commands = ["python agent.py"]

[[panes]]
title = "Shell"
cwd = "~"
```

#### Pane Config Fields

| Field | Type | Description |
|-------|------|-------------|
| `title` | `string` | Title displayed in the pane's title bar |
| `command` | `string` | Shell to run (default: `$SHELL`) |
| `cwd` | `string` | Working directory (supports `~` expansion) |
| `initial_commands` | `[string]` | Commands to run after shell initializes |
| `watermark` | `string` | Large faded text rendered behind terminal content |

Initial commands are sent after the shell has been idle for 500ms, ensuring that shell init scripts (`.zshrc`, `.bashrc`) have finished before commands are dispatched.

## Keyboard Shortcuts

### Pane Navigation

| Shortcut | Action |
|----------|--------|
| `Cmd+1`..`Cmd+9` | Jump to pane by number |
| `Cmd+]` | Focus next pane |
| `Cmd+[` | Focus previous pane |
| Click | Focus clicked pane |

### Pane Management

| Shortcut | Action |
|----------|--------|
| `Cmd+N` | New pane |
| `Cmd+W` | Close focused pane |

### Font

| Shortcut | Action |
|----------|--------|
| `Cmd+=` / `Cmd++` | Increase font size |
| `Cmd+-` | Decrease font size |
| `Cmd+0` | Reset font size |

### Broadcast & Multi-Select

| Shortcut | Action |
|----------|--------|
| `Cmd+Shift+B` | Toggle broadcast mode (type into all panes) |
| `Shift+Click+Drag` | Rectangle-select panes |
| `Cmd+Click` | Toggle a pane in/out of selection |
| `Cmd+Shift+A` | Select all panes |
| `Cmd+Shift+D` | Deselect all panes |

When panes are selected (amber border + checkmark), keyboard input goes to all selected panes. Broadcast mode (green `BC` indicator + green border) sends to every pane regardless of selection.

### Command Overlay

| Shortcut | Action |
|----------|--------|
| `Cmd+Shift+Enter` | Open command overlay |
| `Option+Option` (double-tap) | Open command overlay |
| `Enter` (in overlay) | Send command to targets |
| `Escape` (in overlay) | Cancel overlay |

The command overlay is a quick input bar at the bottom of the screen. Type a command and press Enter to send it to all panes (or only selected panes if any are selected).

### Other

| Shortcut | Action |
|----------|--------|
| `Cmd+,` | Open config file in default editor |
| `Cmd+/` | Toggle keyboard shortcut help |

## Visual Indicators

- **Blue border** -- focused pane
- **Gray border** -- unfocused pane
- **Amber/orange border + checkmark** -- selected pane (multi-select)
- **Green border + "BC" badge** -- broadcast mode active
- **Dimmed content** -- unfocused panes are slightly faded
- **Red tint** -- error detected in pane output (auto-clears after 10s)
- **Watermark text** -- large faded text behind content (configured per-pane)

## Error Detection

Termania automatically scans terminal output for common error patterns:

- `error:`, `error[` (Rust, general)
- `fatal:`, `panic:` (Go, Rust)
- `traceback`, `exception:` (Python)
- `failed:`, `command not found`
- `no such file`, `permission denied`
- `segfault`, `errno`

When detected, the pane gets a subtle red tint overlay. The tint auto-clears 10 seconds after the last error is seen.

## Text Tap API

Termania exposes a Unix domain socket (default: `/tmp/termania.sock`) that external processes can connect to for reading terminal content and sending input to panes.

### Protocol

Newline-delimited JSON over a Unix domain socket.

#### Client to Server

```json
{"subscribe": "all"}
{"subscribe": 0}
{"unsubscribe": 0}
{"list": true}
{"send": 0, "input": "ls -la\r"}
{"send": "all", "input": "echo hello\r"}
```

#### Server to Client

```json
{"pane": 0, "content": "$ ls\nfile1.txt\nfile2.txt\n"}
{"panes": 9}
{"ok": true}
```

### Example: Read Terminal Content

```bash
# Connect and receive all pane updates
echo '{"subscribe": "all"}' | nc -U /tmp/termania.sock

# Send a command to pane 0
echo '{"send": 0, "input": "git status\r"}' | nc -U /tmp/termania.sock

# Send a command to all panes
echo '{"send": "all", "input": "clear\r"}' | nc -U /tmp/termania.sock
```

Broadcasts are throttled to avoid flooding -- content is only sent when it changes or every 250ms.

## Architecture

```
src/
  main.rs        -- Application entry point, event loop, keybindings
  config.rs      -- Configuration structs, TOML loading, session config
  grid.rs        -- Grid layout computation
  pty.rs          -- PTY (pseudo-terminal) management via fork/exec
  renderer.rs    -- GPU rendering with wgpu (glyph atlas, rect/text pipelines)
  terminal.rs    -- VTE terminal emulation, cell grid, escape sequence handling
  text_tap.rs    -- Unix socket server for external process integration
```

### Rendering Pipeline

1. Each pane's terminal cells are collected into `PaneRenderData`
2. The grid layout computes pixel rectangles for each pane
3. Solid rectangles (backgrounds, borders, cursor, overlays) are batched into a rect vertex buffer
4. Text glyphs are rasterized via cosmic-text, cached in a GPU texture atlas, and batched into a text vertex buffer
5. A single render pass draws all rects, then all text, using wgpu

### Terminal Emulation

- VTE parser (`vte` crate) handles escape sequences across read boundaries using a persistent parser state machine
- Supports: cursor movement, scroll regions, alternate screen, SGR attributes (bold, italic, underline, inverse), 256-color and truecolor, OSC title changes, device status reports
- PTY resize uses debounced SIGWINCH (150ms) to prevent flooding during rapid window resizing

## Dependencies

| Crate | Purpose |
|-------|---------|
| `winit` | Window management and event loop |
| `wgpu` | GPU-accelerated rendering (Metal on macOS) |
| `cosmic-text` | Font shaping and glyph rasterization |
| `vte` | Terminal escape sequence parsing |
| `libc` | PTY management (posix_openpt, fork, exec) |
| `serde` + `toml` | Configuration file parsing |
| `dirs` | Platform-appropriate config directory |

## License

MIT

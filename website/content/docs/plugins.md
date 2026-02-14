+++
title = "Plugins"
description = "Reference for all 10 Termania pane plugin types: terminal, webview, notes, screen capture, file browser, process monitor, log viewer, markdown preview, system info, and git status."
weight = 4
+++

Termania supports 10 built-in pane plugin types. Each pane in the grid can be a different type, configured via `[[panes]]` blocks in your session file.

All plugins share these common fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | String | `"terminal"` | The plugin type identifier |
| `title` | String? | Auto-generated | Custom title for the pane's title bar |

---

## terminal

Full VT100/VT220 terminal emulator with a PTY-backed shell. This is the default pane type.

```toml
[[panes]]
type = "terminal"
title = "Dev Shell"
command = "/bin/zsh"
cwd = "~/projects/myapp"
watermark = "DEV"
initial_commands = ["echo 'Hello!'", "ls -la"]
# env = [["NODE_ENV", "development"], ["PORT", "3000"]]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | String? | `$SHELL` | Shell or command to execute |
| `cwd` | String? | Inherited | Working directory (supports `~` expansion) |
| `watermark` | String? | None | Large faded text displayed behind the terminal content, useful for identifying panes at a glance |
| `initial_commands` | String[]? | None | Commands to run automatically after the shell starts. Each is sent with a carriage return |
| `env` | \[String, String\][]? | None | Extra environment variables as `[key, value]` pairs |

**Features:**
- GPU-rendered text via wgpu
- Full ANSI color support (16 colors + 256-color + true color)
- Scrollback history
- Mouse text selection (click-drag, double-click for word selection)
- Copy/paste via system clipboard
- VT100 escape sequence handling via the `vte` crate
- Broadcasts input when broadcast mode is active

---

## webview

Embedded web browser using macOS WKWebView. Click inside the pane to interact -- keyboard input is handled natively by WKWebView when focused.

```toml
[[panes]]
type = "webview"
title = "Documentation"
url = "https://docs.rs"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | String? | None | URL to load in the embedded browser |

**Features:**
- Full web rendering via WebKit
- JavaScript support
- Navigation via the Text Tap API (`navigate` action)
- Native macOS scrolling and interaction
- Automatically positioned and resized with the grid layout

> **Note:** WebView panes are macOS-only and require the `objc2-web-kit` dependency.

---

## notes

Editable text area using macOS NSTextView. Supports both in-memory scratch pads and file-backed persistent notes.

```toml
# In-memory scratch pad
[[panes]]
type = "notes"
title = "Scratch Pad"
content = "Type anything here..."

# File-backed persistent notes
[[panes]]
type = "notes"
title = "TODO List"
file = "~/termania-todo.md"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `content` | String? | None | Initial content (used when no `file` is specified) |
| `file` | String? | None | File path for persistent storage. Content is loaded on startup and auto-saved on close |

**Features:**
- Native macOS text editing
- File persistence (when `file` is specified)
- Content manipulation via the Text Tap API (`set_content` action)

> **Note:** Notes panes are macOS-only and require the `objc2-app-kit` dependency.

---

## screen_capture

Mirrors another application's window into a Termania pane. Uses macOS ScreenCaptureKit.

```toml
# By application bundle ID
[[panes]]
type = "screen_capture"
title = "Safari Mirror"
target = "com.apple.Safari"

# By window title
[[panes]]
type = "screen_capture"
title = "VS Code Mirror"
target_title = "termania"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `target` | String? | None | Application bundle ID to capture (e.g., `"com.apple.Safari"`) |
| `target_title` | String? | None | Target a specific window by its title string |

> **Note:** This plugin is a work in progress. macOS-only.

---

## file_browser

Interactive file browser that displays a directory tree.

```toml
[[panes]]
type = "file_browser"
title = "Project Files"
path = "~/projects/myapp"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | String? | `"."` | Root directory to browse (supports `~` expansion) |

**Features:**
- Tree view with expandable directories
- File size display
- Keyboard navigation (arrow keys, Enter to expand/collapse)
- Auto-refresh on changes

---

## process_monitor

Displays a list of running processes sorted by CPU usage, similar to `top`.

```toml
[[panes]]
type = "process_monitor"
title = "Processes"
refresh_ms = 2000
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `refresh_ms` | Integer? | `2000` | Refresh interval in milliseconds |

**Features:**
- PID, user, CPU %, memory %, and command columns
- Sorted by CPU usage
- Scrollable list
- Auto-refresh at the configured interval

---

## log_viewer

Tails a log file and displays its contents with auto-scroll.

```toml
[[panes]]
type = "log_viewer"
title = "System Log"
file = "/var/log/system.log"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `file` | String? | `"/var/log/system.log"` | Path to the log file to tail. Also accepts `path` as an alias |

**Features:**
- Tails the file (reads new content as it appears)
- Scrollable history (up to 10,000 lines)
- Incremental reads (efficient for large log files)

---

## markdown_preview

Renders Markdown content with syntax highlighting for headings, code blocks, lists, and emphasis.

```toml
# From a file
[[panes]]
type = "markdown_preview"
title = "README"
file = "~/projects/myapp/README.md"

# Inline content
[[panes]]
type = "markdown_preview"
title = "Preview"
content = "# Hello\n\nThis is **bold** text."
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `file` | String? | None | Markdown file to render. Also accepts `path` as an alias |
| `content` | String? | None | Inline Markdown content (used when no `file` is specified) |

**Features:**
- Heading rendering with color differentiation
- Code block highlighting
- Bold and italic text support
- Scrollable content

---

## system_info

Displays live system information including hostname, OS version, CPU, memory, disk, and network statistics.

```toml
[[panes]]
type = "system_info"
title = "System"
refresh_ms = 3000
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `refresh_ms` | Integer? | `3000` | Refresh interval in milliseconds |

**Features:**
- Hostname and OS version
- System uptime and load averages
- CPU usage percentage
- Memory usage
- Disk usage
- Network interface information

---

## git_status

Displays Git repository status, including branch, modified files, and recent log.

```toml
[[panes]]
type = "git_status"
title = "Git"
repo = "~/projects/myapp"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `repo` | String? | `"."` | Path to the Git repository. Also accepts `path` as an alias |

**Features:**
- Current branch display
- Modified/staged/untracked file listing with status indicators
- Recent commit log
- Multiple views: Status, Log, Diff
- Auto-refresh on changes

---

## Mixing Plugin Types

You can combine any plugin types in a single session. Here is an example that uses five different types in a 2x3 grid:

```toml
title = "Full Stack Dev"
rows = 2
cols = 3

[[panes]]
type = "terminal"
title = "Shell"
cwd = "~/projects/myapp"
watermark = "DEV"

[[panes]]
type = "terminal"
title = "Server"
initial_commands = ["npm run dev"]

[[panes]]
type = "webview"
title = "Preview"
url = "http://localhost:3000"

[[panes]]
type = "file_browser"
title = "Files"
path = "~/projects/myapp"

[[panes]]
type = "git_status"
title = "Git"
repo = "~/projects/myapp"

[[panes]]
type = "system_info"
title = "System"
```

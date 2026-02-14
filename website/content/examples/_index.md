+++
title = "Examples"
template = "section.html"
+++

Example session files to get you started with Termania. These files are also available in the `examples/` directory of the [source repository](https://github.com/anthropics/termania).

## Basic: Single Terminal

The simplest possible setup -- a single terminal pane with default settings:

```toml
# termania.toml
[[panes]]
title = "Shell"
```

## Development Workflow

A 2x2 grid with an editor, dev server, test runner, and embedded preview:

```toml
title = "Dev Workflow"
rows = 2
cols = 2

[[panes]]
title = "Editor"
cwd = "~/projects/myapp"
watermark = "EDIT"

[[panes]]
title = "Server"
cwd = "~/projects/myapp"
initial_commands = ["npm run dev"]
watermark = "SRV"

[[panes]]
title = "Tests"
cwd = "~/projects/myapp"
initial_commands = ["cargo watch -x test"]
watermark = "TEST"

[[panes]]
type = "webview"
title = "Preview"
url = "http://localhost:3000"
```

## Multi-LLM Grid

A 3x3 grid running multiple LLM sessions simultaneously, with broadcast mode to send the same prompt to all:

```toml
title = "LLM Grid"
rows = 3
cols = 3

[[panes]]
title = "Claude 1"
initial_commands = ["claude"]

[[panes]]
title = "Claude 2"
initial_commands = ["claude"]

[[panes]]
title = "Claude 3"
initial_commands = ["claude"]

[[panes]]
title = "Claude 4"
initial_commands = ["claude"]

[[panes]]
title = "Claude 5"
initial_commands = ["claude"]

[[panes]]
title = "Claude 6"
initial_commands = ["claude"]

[[panes]]
title = "Claude 7"
initial_commands = ["claude"]

[[panes]]
title = "Claude 8"
initial_commands = ["claude"]

[[panes]]
title = "Claude 9"
initial_commands = ["claude"]
```

Use `Cmd+Shift+B` to enable broadcast mode and send the same prompt to all 9 instances at once.

## Plugin Gallery

Showcases multiple plugin types in a 2x3 grid:

```toml
title = "Plugin Gallery"
rows = 2
cols = 3

[[panes]]
type = "terminal"
title = "Shell"
watermark = "DEV"
cwd = "~"

[[panes]]
type = "terminal"
title = "Build"
watermark = "BUILD"

[[panes]]
type = "webview"
title = "Docs"
url = "https://docs.rs"

[[panes]]
type = "notes"
title = "Scratch"
content = "# Notes\n\nType anything here."

[[panes]]
type = "git_status"
title = "Git"
repo = "~/projects/myapp"

[[panes]]
type = "system_info"
title = "System"
```

## Monitoring Dashboard

A 2x3 dashboard for system and application monitoring:

```toml
title = "Monitor"
rows = 2
cols = 3

[[panes]]
type = "system_info"
title = "System"
refresh_ms = 2000

[[panes]]
type = "process_monitor"
title = "Processes"
refresh_ms = 1500

[[panes]]
type = "log_viewer"
title = "System Log"
file = "/var/log/system.log"

[[panes]]
type = "terminal"
title = "Shell"
watermark = "OPS"

[[panes]]
type = "git_status"
title = "Deploy Repo"
repo = "~/deploy"

[[panes]]
type = "file_browser"
title = "Files"
path = "~/projects"
```

## Full Stack with File Browser

Combines terminals, documentation, file browsing, and Markdown preview:

```toml
title = "Full Stack"
rows = 2
cols = 3

[[panes]]
type = "terminal"
title = "Frontend"
cwd = "~/projects/myapp/frontend"
initial_commands = ["npm run dev"]

[[panes]]
type = "terminal"
title = "Backend"
cwd = "~/projects/myapp/backend"
initial_commands = ["cargo watch -x run"]

[[panes]]
type = "webview"
title = "App"
url = "http://localhost:5173"

[[panes]]
type = "file_browser"
title = "Files"
path = "~/projects/myapp"

[[panes]]
type = "markdown_preview"
title = "README"
file = "~/projects/myapp/README.md"

[[panes]]
type = "git_status"
title = "Git"
repo = "~/projects/myapp"
```

+++
title = "Configuration"
description = "Complete reference for Termania's TOML configuration files: config.toml and termania.toml."
weight = 2
+++

Termania uses two TOML files for configuration:

- **`config.toml`** -- Global appearance settings (font, colors, grid defaults, LLM, Text Tap)
- **`termania.toml`** -- Session/project layout (grid dimensions, pane definitions)

## File Locations

### config.toml (global)

Looked up in this order:

1. `~/.config/termania/config.toml`
2. `$XDG_CONFIG_HOME/termania/config.toml`
3. Platform-native config dir (e.g., `~/Library/Application Support/termania/config.toml` on macOS)

A default `config.toml` is created automatically on first launch.

### termania.toml (session)

Looked up in this order:

1. Explicit CLI argument: `termania path/to/session.toml`
2. `./termania.toml` in the current working directory
3. `~/.config/termania/termania.toml`
4. Falls back to `[[panes]]` in `config.toml`

This means you can drop a `termania.toml` in any project directory and Termania will automatically load it when launched from that directory.

---

## \[font\]

Typography settings for all terminal panes. Only in `config.toml`.

```toml
[font]
family = "SF Mono"         # Font family name
size = 14.0                # Font size in points
line_height = 1.2          # Line height multiplier (1.0 = tight)
letter_spacing = 0.0       # Extra horizontal spacing in pixels
# bold_family = "Menlo"    # Optional: separate font for bold text
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `family` | String | `"SF Mono"` | Font family name. Examples: `"JetBrains Mono"`, `"Menlo"`, `"Fira Code"`, `"Cascadia Code"` |
| `size` | Float | `14.0` | Font size in points |
| `line_height` | Float | `1.2` | Line height multiplier |
| `letter_spacing` | Float | `0.0` | Extra character spacing in pixels |
| `bold_family` | String? | `None` | Optional separate font family for bold text |

---

## \[grid\]

Grid layout for the pane tiling system. Only in `config.toml` (but `rows`/`cols` can be overridden by session files).

```toml
[grid]
rows = 3
cols = 3
gap = 4
inner_padding = 4
outer_padding = 4
title_bar_height = 24
border_radius = 8
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `rows` | Integer | `1` | Number of rows in the pane grid |
| `cols` | Integer | `1` | Number of columns in the pane grid |
| `gap` | Integer | `4` | Gap between panes in pixels |
| `inner_padding` | Integer | `4` | Padding inside each pane (border to content) |
| `outer_padding` | Integer | `4` | Padding around the entire grid |
| `title_bar_height` | Integer | `24` | Height of each pane's title bar in pixels |
| `border_radius` | Integer | `8` | Corner radius for pane borders (0 = sharp) |

---

## \[window\]

Initial window dimensions and title. Only in `config.toml` (title can be overridden by session).

```toml
[window]
width = 1920
height = 1080
title = "Termania"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `width` | Integer | `1920` | Initial window width in pixels |
| `height` | Integer | `1080` | Initial window height in pixels |
| `title` | String | `"Termania"` | Window title (overridden by session-level `title`) |

---

## \[colors\]

Color scheme for all terminal panes. All values are hex strings (`#RRGGBB` or `#RRGGBBAA`). Only in `config.toml`.

```toml
[colors]
background = "#010409"
foreground = "#e6edf3"
cursor = "#f0f6fc"
selection = "#264f78"
border = "#30363d"
border_focused = "#58a6ff"
title_bg = "#0d1117"
title_fg = "#e6edf3"

ansi = [
    "#0d1117",  # 0  black
    "#ff7b72",  # 1  red
    "#3fb950",  # 2  green
    "#d29922",  # 3  yellow
    "#58a6ff",  # 4  blue
    "#bc8cff",  # 5  magenta
    "#39d353",  # 6  cyan
    "#c9d1d9",  # 7  white
    "#484f58",  # 8  bright black
    "#ffa198",  # 9  bright red
    "#56d364",  # 10 bright green
    "#e3b341",  # 11 bright yellow
    "#79c0ff",  # 12 bright blue
    "#d2a8ff",  # 13 bright magenta
    "#56d364",  # 14 bright cyan
    "#f0f6fc",  # 15 bright white
]
```

| Key | Default | Description |
|-----|---------|-------------|
| `background` | `#010409` | Background color for all panes |
| `foreground` | `#e6edf3` | Default text/foreground color |
| `cursor` | `#f0f6fc` | Cursor block color |
| `selection` | `#264f78` | Text selection highlight |
| `border` | `#30363d` | Unfocused pane border |
| `border_focused` | `#58a6ff` | Focused pane border (bright blue) |
| `title_bg` | `#0d1117` | Title bar background |
| `title_fg` | `#e6edf3` | Title bar text |
| `ansi` | (16 colors) | ANSI color palette: 8 normal + 8 bright |

---

## \[text_tap\]

The Text Tap API server. See the [Text Tap API](/docs/text-tap-api/) reference for protocol details. Only in `config.toml`.

```toml
[text_tap]
enabled = true
socket_path = "/tmp/termania.sock"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable/disable the Unix socket server |
| `socket_path` | String | `"/tmp/termania.sock"` | Path for the Unix domain socket |

---

## \[llm\]

LLM integration for the command overlay (`Cmd+Shift+Enter`). Supports Anthropic, OpenAI, Ollama, and custom OpenAI-compatible endpoints. Only in `config.toml`.

```toml
[llm]
provider = "anthropic"
# api_key = "sk-..."          # Or set ANTHROPIC_API_KEY env var
# model = "claude-sonnet-4-20250514"
# base_url = "https://..."    # For ollama/custom providers
max_tokens = 1024
# system_prompt = "You are a helpful terminal assistant."
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `provider` | String | `"anthropic"` | LLM provider: `"anthropic"`, `"openai"`, `"ollama"`, `"custom"` |
| `api_key` | String? | `None` | API key (falls back to `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` env vars) |
| `model` | String? | `None` | Model name. Defaults: Anthropic = `claude-sonnet-4-20250514`, OpenAI = `gpt-4o`, Ollama = `llama3` |
| `base_url` | String? | `None` | Base URL for Ollama/custom endpoints. Ollama default: `http://localhost:11434/v1` |
| `max_tokens` | Integer | `1024` | Maximum tokens in the LLM response |
| `system_prompt` | String? | `None` | Custom system prompt override |

---

## \[\[panes\]\]

Pane definitions. Each `[[panes]]` block defines one pane in the grid. Panes fill the grid left-to-right, top-to-bottom.

Panes can appear in session files (`termania.toml`) or as a fallback in `config.toml`.

### Common fields

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `type` | String | `"terminal"` | Pane type (see [Plugins](/docs/plugins/)) |
| `title` | String? | Auto-generated | Custom title for the pane's title bar |

### Terminal pane fields

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

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | String? | `$SHELL` | Shell or command to run |
| `cwd` | String? | Inherited | Working directory (supports `~` expansion) |
| `watermark` | String? | `None` | Large faded background text |
| `initial_commands` | String[]? | `None` | Commands to run on startup |
| `env` | [String, String][]? | `None` | Extra environment variables |

### WebView pane fields

```toml
[[panes]]
type = "webview"
title = "Docs"
url = "https://docs.rs"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `url` | String? | `None` | URL to load in the embedded browser |

### Notes pane fields

```toml
[[panes]]
type = "notes"
title = "Scratch"
content = "Type here..."
# file = "~/notes.md"    # For persistent notes
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `content` | String? | `None` | Initial in-memory content |
| `file` | String? | `None` | File path for persistent storage (auto-saved) |

### Other plugin types

See the [Plugins](/docs/plugins/) reference for full details on all 10 plugin types and their specific configuration fields.

---

## Session-Level Overrides

Session files (`termania.toml`) support these top-level keys that override `config.toml`:

```toml
title = "My Project"     # Overrides [window] title
rows = 2                 # Overrides [grid] rows
cols = 3                 # Overrides [grid] cols

[[panes]]
# ... pane definitions
```

## CLI Usage

```sh
termania                          # Default config/session
termania path/to/session.toml    # Specific session file
termania -h / --help              # Print help
termania -v / --version           # Print version
```

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log level (e.g., `RUST_LOG=debug termania`) |
| `SHELL` | Default shell for terminal panes |
| `ANTHROPIC_API_KEY` | Anthropic API key (fallback for `[llm]`) |
| `OPENAI_API_KEY` | OpenAI API key (fallback for `[llm]`) |

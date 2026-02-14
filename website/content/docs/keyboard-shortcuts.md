+++
title = "Keyboard Shortcuts"
description = "Complete list of keyboard shortcuts for Termania."
weight = 3
+++

All keyboard shortcuts are hardcoded and cannot be customized via configuration. On macOS, `Cmd` refers to the Command key.

## Pane Management

| Shortcut | Action |
|----------|--------|
| `Cmd+N` | Add a new pane to the right in the current row |
| `Cmd+Option+N` | Add a new row with one pane |
| `Cmd+Shift+N` | Open a new window |
| `Cmd+W` | Close the focused pane |
| `Cmd+R` | Rename the focused pane (opens rename overlay) |

## Pane Navigation

| Shortcut | Action |
|----------|--------|
| `Cmd+1` through `Cmd+9` | Jump to pane by number (1-indexed) |
| `Cmd+]` | Focus the next pane |
| `Cmd+[` | Focus the previous pane |
| `Cmd+Shift+Left` | Swap focused pane with the neighbor to the left |
| `Cmd+Shift+Right` | Swap focused pane with the neighbor to the right |
| `Cmd+Shift+Up` | Swap focused pane with the neighbor above |
| `Cmd+Shift+Down` | Swap focused pane with the neighbor below |

## Font Size

| Shortcut | Action |
|----------|--------|
| `Cmd++` / `Cmd+=` | Increase font size by 2pt |
| `Cmd+-` | Decrease font size by 2pt |
| `Cmd+0` | Reset font size to the configured default |

## Broadcast and Multi-Select

| Shortcut | Action |
|----------|--------|
| `Cmd+Shift+B` | Toggle broadcast mode (type in all terminal panes at once) |
| `Shift+Click+Drag` | Rectangle-select multiple panes |
| `Cmd+Click` | Toggle a single pane in/out of the selection |
| `Cmd+Shift+A` | Select all panes |
| `Cmd+Shift+D` | Deselect all panes |

When multiple panes are selected, the command overlay targets only those panes.

## Command Overlay

| Shortcut | Action |
|----------|--------|
| `Cmd+Shift+Enter` | Open the command overlay in LLM mode |
| `Option+Option` | Double-tap Option within 400ms to open the command overlay |

### Inside the Command Overlay

| Shortcut | Action |
|----------|--------|
| `Escape` | Close the overlay |
| `Enter` | Submit the command / confirm the LLM response |
| `Backspace` | Delete last character |
| `Space` | Toggle pane selection (when in target mode) |

## Rename Overlay

| Shortcut | Action |
|----------|--------|
| `Escape` | Cancel rename |
| `Enter` | Confirm rename |
| `Backspace` | Delete last character |

## Help Overlay

| Shortcut | Action |
|----------|--------|
| `Cmd+/` or `Cmd+?` | Toggle the help overlay |
| `Escape` | Close the help overlay |
| `Up` / `k` | Scroll up |
| `Down` / `j` | Scroll down |
| `Page Up` | Scroll up 10 lines |
| `Page Down` | Scroll down 10 lines |
| `Home` | Scroll to top |

## Text Selection

| Shortcut | Action |
|----------|--------|
| `Click+Drag` | Select text in a terminal pane |
| `Double-Click` | Select a word |
| `Escape` | Clear the current selection |

## Other

| Shortcut | Action |
|----------|--------|
| `Cmd+,` | Open `config.toml` in your default editor |

## Terminal Input

Standard terminal key sequences are forwarded to the focused pane:

- `Enter`, `Backspace`, `Tab`, `Escape`
- Arrow keys (`Up`, `Down`, `Left`, `Right`)
- `Home`, `End`, `Page Up`, `Page Down`
- `Insert`, `Delete`
- `F1` through `F12`
- `Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, and all other Ctrl combinations

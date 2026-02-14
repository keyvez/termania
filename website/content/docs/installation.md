+++
title = "Installation"
description = "How to install Termania on macOS from Cargo or by building from source."
weight = 1
+++

## Requirements

- **macOS** (Termania uses macOS-native APIs for WebView, Notes, and Screen Capture plugins)
- **Rust 1.75+** (edition 2021)
- **GPU** with Metal support (required by wgpu)

## Install from Cargo

The simplest way to install Termania is via Cargo:

```sh
cargo install termania
```

This downloads the crate from [crates.io](https://crates.io/crates/termania), compiles it with release optimizations (LTO enabled), and places the `termania` binary in `~/.cargo/bin/`.

Make sure `~/.cargo/bin` is in your `PATH`:

```sh
# Add to your ~/.zshrc or ~/.bashrc
export PATH="$HOME/.cargo/bin:$PATH"
```

## Build from Source

Clone the repository and build with release optimizations:

```sh
git clone https://github.com/anthropics/termania.git
cd termania
cargo build --release
```

The binary will be at `target/release/termania`. You can copy it to a location in your `PATH`:

```sh
cp target/release/termania /usr/local/bin/
```

## Dependencies

Termania's dependencies are managed entirely through Cargo. Key runtime dependencies include:

| Crate | Purpose |
|-------|---------|
| `wgpu` | GPU-accelerated rendering via Metal |
| `winit` | Window creation and event loop |
| `cosmic-text` | Font shaping and text layout |
| `vte` | VT100/VT220 terminal escape sequence parsing |
| `objc2` / `objc2-web-kit` | macOS native views (WKWebView, NSTextView) |
| `ureq` | HTTP client for LLM API calls |
| `arboard` | Clipboard support (copy/paste) |
| `serde` / `toml` | Configuration file parsing |

No system libraries need to be installed separately; the macOS SDK provides everything else.

## Verify Installation

```sh
termania --version
```

This prints the version number and exits.

## First Launch

Simply run `termania` with no arguments to start with a single terminal pane using the default configuration:

```sh
termania
```

Termania automatically creates a default config file at `~/.config/termania/config.toml` on first run.

To launch with a specific session file:

```sh
termania path/to/session.toml
```

See the [Configuration](/docs/configuration/) guide for details on customizing your setup.

## Uninstall

If installed via Cargo:

```sh
cargo uninstall termania
```

To also remove configuration files:

```sh
rm -rf ~/.config/termania
```

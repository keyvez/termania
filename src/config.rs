use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level configuration for Termania
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub grid: GridConfig,
    pub window: WindowConfig,
    pub colors: ColorConfig,
    pub text_tap: TextTapConfig,
    pub panes: Vec<PaneConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font family name (e.g., "JetBrains Mono", "Menlo", "SF Mono")
    pub family: String,
    /// Font size in points
    pub size: f32,
    /// Bold font weight
    pub bold_family: Option<String>,
    /// Line height multiplier (1.0 = normal)
    pub line_height: f32,
    /// Letter spacing in pixels
    pub letter_spacing: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GridConfig {
    /// Number of rows in the grid
    pub rows: usize,
    /// Number of columns in the grid
    pub cols: usize,
    /// Gap between panes in pixels
    pub gap: u32,
    /// Padding inside each pane in pixels
    pub inner_padding: u32,
    /// Padding around the entire grid in pixels
    pub outer_padding: u32,
    /// Height of the title bar for each pane
    pub title_bar_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Initial window width
    pub width: u32,
    /// Initial window height
    pub height: u32,
    /// Window title
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    /// Background color (hex)
    pub background: String,
    /// Foreground/text color (hex)
    pub foreground: String,
    /// Cursor color (hex)
    pub cursor: String,
    /// Selection color (hex)
    pub selection: String,
    /// Border color for panes (hex)
    pub border: String,
    /// Focused pane border color (hex)
    pub border_focused: String,
    /// Title bar background (hex)
    pub title_bg: String,
    /// Title bar text color (hex)
    pub title_fg: String,
    /// ANSI color palette (16 colors)
    pub ansi: [String; 16],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TextTapConfig {
    /// Unix domain socket path for text tap
    pub socket_path: String,
    /// Whether the text tap server is enabled
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneConfig {
    /// Pane type: "terminal" (default), "webview", "notes", "screen_capture"
    #[serde(rename = "type", default = "default_pane_type")]
    pub pane_type: String,
    /// Custom title for this pane
    pub title: Option<String>,
    /// Command to run (defaults to $SHELL) — terminal plugin
    pub command: Option<String>,
    /// Working directory — terminal plugin
    pub cwd: Option<String>,
    /// Environment variables — terminal plugin
    pub env: Option<Vec<(String, String)>>,
    /// Commands to run on startup (each sent as input with \r) — terminal plugin
    pub initial_commands: Option<Vec<String>>,
    /// Large faded watermark text behind terminal content — terminal plugin
    pub watermark: Option<String>,
    /// URL to load — webview plugin
    pub url: Option<String>,
    /// File path for persistent content — notes plugin
    pub file: Option<String>,
    /// Initial content for notes plugin (when no file is specified)
    pub content: Option<String>,
    /// Target app bundle ID — screen_capture plugin
    pub target: Option<String>,
    /// Target window title — screen_capture plugin
    pub target_title: Option<String>,
}

fn default_pane_type() -> String {
    "terminal".to_string()
}

/// Session configuration loaded from sessions.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Window title (overrides config.toml)
    pub title: Option<String>,
    /// Grid rows (overrides config.toml)
    pub rows: Option<usize>,
    /// Grid cols (overrides config.toml)
    pub cols: Option<usize>,
    /// Pane definitions
    pub panes: Vec<PaneConfig>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            title: None,
            rows: None,
            cols: None,
            panes: Vec::new(),
        }
    }
}

impl SessionConfig {
    /// Load session config from an explicit path, or fall back to the default
    /// lookup order: ./termania.toml → ~/.config/termania/termania.toml.
    pub fn load(explicit_path: Option<&str>) -> Option<Self> {
        // If an explicit path was provided, use it (and fail loudly if invalid)
        if let Some(path) = explicit_path {
            let path = PathBuf::from(expand_tilde(path));
            if !path.exists() {
                log::error!("Session file not found: {}", path.display());
                eprintln!("error: session file not found: {}", path.display());
                std::process::exit(1);
            }
            return Self::load_from(&path);
        }

        // 1. Check ./termania.toml (current working directory)
        let local_path = PathBuf::from("termania.toml");
        if local_path.exists() {
            if let Some(session) = Self::load_from(&local_path) {
                return Some(session);
            }
        }

        // 2. Check ~/.config/termania/termania.toml
        let global_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("termania")
            .join("termania.toml");
        if global_path.exists() {
            if let Some(session) = Self::load_from(&global_path) {
                return Some(session);
            }
        }

        None
    }

    fn load_from(path: &PathBuf) -> Option<Self> {
        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(session) => {
                    log::info!("Loaded session config from {}", path.display());
                    Some(session)
                }
                Err(e) => {
                    log::warn!("Failed to parse session config {}: {}", path.display(), e);
                    None
                }
            },
            Err(e) => {
                log::warn!("Failed to read session config {}: {}", path.display(), e);
                None
            }
        }
    }
}

// Defaults

impl Default for Config {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            grid: GridConfig::default(),
            window: WindowConfig::default(),
            colors: ColorConfig::default(),
            text_tap: TextTapConfig::default(),
            panes: Vec::new(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "SF Mono".to_string(),
            size: 14.0,
            bold_family: None,
            line_height: 1.2,
            letter_spacing: 0.0,
        }
    }
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            rows: 3,
            cols: 3,
            gap: 4,
            inner_padding: 4,
            outer_padding: 4,
            title_bar_height: 24,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            title: "Termania".to_string(),
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            background: "#010409".to_string(),
            foreground: "#e6edf3".to_string(),
            cursor: "#f0f6fc".to_string(),
            selection: "#264f78".to_string(),
            border: "#30363d".to_string(),
            border_focused: "#58a6ff".to_string(),
            title_bg: "#0d1117".to_string(),
            title_fg: "#e6edf3".to_string(),
            ansi: [
                // Normal colors
                "#0d1117".to_string(), // black
                "#ff7b72".to_string(), // red
                "#3fb950".to_string(), // green
                "#d29922".to_string(), // yellow
                "#58a6ff".to_string(), // blue
                "#bc8cff".to_string(), // magenta
                "#39d353".to_string(), // cyan
                "#c9d1d9".to_string(), // white
                // Bright colors
                "#484f58".to_string(), // bright black
                "#ffa198".to_string(), // bright red
                "#56d364".to_string(), // bright green
                "#e3b341".to_string(), // bright yellow
                "#79c0ff".to_string(), // bright blue
                "#d2a8ff".to_string(), // bright magenta
                "#56d364".to_string(), // bright cyan
                "#f0f6fc".to_string(), // bright white
            ],
        }
    }
}

impl Default for TextTapConfig {
    fn default() -> Self {
        Self {
            socket_path: "/tmp/termania.sock".to_string(),
            enabled: true,
        }
    }
}

impl Config {
    /// Load config from ~/.config/termania/config.toml, falling back to defaults
    pub fn load() -> Self {
        let config_path = Self::config_path();
        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(config) => {
                        log::info!("Loaded config from {}", config_path.display());
                        return config;
                    }
                    Err(e) => {
                        log::warn!("Failed to parse config: {}, using defaults", e);
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read config: {}, using defaults", e);
                }
            }
        } else {
            log::info!("No config file found at {}, using defaults", config_path.display());
            // Write default config for reference
            let config = Config::default();
            if let Some(parent) = config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(toml_str) = toml::to_string_pretty(&config) {
                let _ = std::fs::write(&config_path, toml_str);
                log::info!("Wrote default config to {}", config_path.display());
            }
        }
        Config::default()
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("termania")
            .join("config.toml")
    }

    /// Get effective window title, preferring session override
    pub fn effective_title(&self, session: Option<&SessionConfig>) -> String {
        session
            .and_then(|s| s.title.clone())
            .unwrap_or_else(|| self.window.title.clone())
    }

    /// Get effective grid rows, preferring session override
    pub fn effective_rows(&self, session: Option<&SessionConfig>) -> usize {
        session
            .and_then(|s| s.rows)
            .unwrap_or(self.grid.rows)
    }

    /// Get effective grid cols, preferring session override
    pub fn effective_cols(&self, session: Option<&SessionConfig>) -> usize {
        session
            .and_then(|s| s.cols)
            .unwrap_or(self.grid.cols)
    }

    /// Get effective panes, preferring session panes, then config panes
    pub fn effective_panes<'a>(&'a self, session: Option<&'a SessionConfig>) -> &'a [PaneConfig] {
        if let Some(s) = session {
            if !s.panes.is_empty() {
                return &s.panes;
            }
        }
        &self.panes
    }
}

/// Expand `~` at the start of a path to the user's home directory
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| format!("{}/{}", h.to_string_lossy(), rest))
            .unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    }
}

/// Parse a hex color string into [f32; 4] RGBA
pub fn parse_hex_color(hex: &str) -> [f32; 4] {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f32 / 255.0;
    let a = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..8], 16).unwrap_or(255) as f32 / 255.0
    } else {
        1.0
    };
    [r, g, b, a]
}

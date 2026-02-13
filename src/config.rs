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
    /// Custom title for this pane
    pub title: Option<String>,
    /// Command to run (defaults to $SHELL)
    pub command: Option<String>,
    /// Working directory
    pub cwd: Option<String>,
    /// Environment variables
    pub env: Option<Vec<(String, String)>>,
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
            family: "Menlo".to_string(),
            size: 18.0,
            bold_family: None,
            line_height: 1.2,
            letter_spacing: 0.0,
        }
    }
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            rows: 2,
            cols: 2,
            gap: 4,
            inner_padding: 8,
            outer_padding: 8,
            title_bar_height: 28,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1600,
            height: 1000,
            title: "Termania".to_string(),
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            background: "#1a1b26".to_string(),
            foreground: "#c0caf5".to_string(),
            cursor: "#c0caf5".to_string(),
            selection: "#33467c".to_string(),
            border: "#3b4261".to_string(),
            border_focused: "#7aa2f7".to_string(),
            title_bg: "#24283b".to_string(),
            title_fg: "#a9b1d6".to_string(),
            ansi: [
                // Normal colors
                "#15161e".to_string(), // black
                "#f7768e".to_string(), // red
                "#9ece6a".to_string(), // green
                "#e0af68".to_string(), // yellow
                "#7aa2f7".to_string(), // blue
                "#bb9af7".to_string(), // magenta
                "#7dcfff".to_string(), // cyan
                "#a9b1d6".to_string(), // white
                // Bright colors
                "#414868".to_string(), // bright black
                "#f7768e".to_string(), // bright red
                "#9ece6a".to_string(), // bright green
                "#e0af68".to_string(), // bright yellow
                "#7aa2f7".to_string(), // bright blue
                "#bb9af7".to_string(), // bright magenta
                "#7dcfff".to_string(), // bright cyan
                "#c0caf5".to_string(), // bright white
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

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("termania")
            .join("config.toml")
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

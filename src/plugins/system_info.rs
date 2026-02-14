use std::process::Command;
use std::time::Instant;

use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

pub struct SystemInfoPlugin {
    title: String,
    cols: usize,
    rows: usize,
    dirty: bool,
    last_refresh: Instant,
    refresh_ms: u64,
    info: SysInfo,
}

#[derive(Default)]
struct SysInfo {
    hostname: String,
    os_version: String,
    uptime: String,
    cpu_usage: String,
    memory: String,
    disk: String,
    load_avg: String,
    network: Vec<String>,
}

impl SystemInfoPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("System {}", index + 1));
        let refresh_ms = pane_config.and_then(|p| p.refresh_ms).unwrap_or(3000);

        let mut plugin = Self {
            title,
            cols: 80,
            rows: 24,
            dirty: true,
            last_refresh: Instant::now(),
            refresh_ms,
            info: SysInfo::default(),
        };
        plugin.refresh();
        plugin
    }

    fn refresh(&mut self) {
        self.info.hostname = run_cmd("hostname", &[]);
        self.info.os_version = run_cmd("sw_vers", &["-productVersion"]);
        self.info.uptime = run_cmd("uptime", &[]);
        self.info.load_avg = self.extract_load_avg();
        self.info.cpu_usage = self.get_cpu_usage();
        self.info.memory = self.get_memory();
        self.info.disk = self.get_disk();
        self.info.network = self.get_network();
        self.dirty = true;
    }

    fn extract_load_avg(&self) -> String {
        let uptime = &self.info.uptime;
        if let Some(idx) = uptime.find("load average") {
            uptime[idx..].trim().to_string()
        } else if let Some(idx) = uptime.find("load averages") {
            uptime[idx..].trim().to_string()
        } else {
            String::new()
        }
    }

    fn get_cpu_usage(&self) -> String {
        let output = run_cmd("top", &["-l", "1", "-n", "0", "-s", "0"]);
        for line in output.lines() {
            if line.contains("CPU usage") {
                return line.trim().to_string();
            }
        }
        "N/A".to_string()
    }

    fn get_memory(&self) -> String {
        let output = run_cmd("vm_stat", &[]);
        let mut free = 0u64;
        let mut active = 0u64;
        let mut inactive = 0u64;
        let mut wired = 0u64;
        let page_size = 16384u64; // Apple Silicon default

        for line in output.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() == 2 {
                let val: u64 = parts[1].trim().trim_end_matches('.').parse().unwrap_or(0);
                if line.contains("Pages free") {
                    free = val;
                } else if line.contains("Pages active") {
                    active = val;
                } else if line.contains("Pages inactive") {
                    inactive = val;
                } else if line.contains("Pages wired") {
                    wired = val;
                }
            }
        }

        let total_pages = free + active + inactive + wired;
        let used_pages = active + wired;
        let total_gb = (total_pages * page_size) as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_gb = (used_pages * page_size) as f64 / (1024.0 * 1024.0 * 1024.0);

        format!("{:.1}G / {:.1}G ({:.0}%)", used_gb, total_gb, if total_gb > 0.0 { used_gb / total_gb * 100.0 } else { 0.0 })
    }

    fn get_disk(&self) -> String {
        let output = run_cmd("df", &["-h", "/"]);
        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                return format!("{} used / {} total ({})", parts[2], parts[1], parts[4]);
            }
        }
        "N/A".to_string()
    }

    fn get_network(&self) -> Vec<String> {
        let output = run_cmd("ifconfig", &[]);
        let mut interfaces = Vec::new();
        let mut current_iface = String::new();

        for line in output.lines() {
            if !line.starts_with('\t') && !line.starts_with(' ') && line.contains(':') {
                current_iface = line.split(':').next().unwrap_or("").to_string();
            } else if line.contains("inet ") && !line.contains("127.0.0.1") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    interfaces.push(format!("{}: {}", current_iface, parts[1]));
                }
            }
        }
        interfaces
    }

    fn make_section_header(&self, text: &str) -> Vec<Cell> {
        let mut cells: Vec<Cell> = format!(" {} ", text).chars().take(self.cols).map(|ch| Cell {
            ch,
            fg: CellColor::Ansi(5), // magenta
            bg: CellColor::Default,
            bold: true,
            italic: false,
            underline: true,
            inverse: false,
        }).collect();
        cells.resize(self.cols, Cell::default());
        cells
    }

    fn make_kv_line(&self, key: &str, value: &str) -> Vec<Cell> {
        let line = format!("  {:>12}: {}", key, value);
        let mut cells: Vec<Cell> = Vec::new();
        for (i, ch) in line.chars().take(self.cols).enumerate() {
            let fg = if i <= 14 {
                CellColor::Ansi(6) // cyan for keys
            } else {
                CellColor::Default
            };
            cells.push(Cell {
                ch,
                fg,
                bg: CellColor::Default,
                bold: i <= 14,
                italic: false,
                underline: false,
                inverse: false,
            });
        }
        cells.resize(self.cols, Cell::default());
        cells
    }

    fn make_bar(&self, label: &str, pct: f64) -> Vec<Cell> {
        let bar_width = self.cols.saturating_sub(20);
        let filled = (pct / 100.0 * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);

        let line = format!(
            "  {:>12}: [{}{}] {:>3.0}%",
            label,
            "\u{2588}".repeat(filled),
            "\u{2591}".repeat(empty),
            pct
        );

        let color = if pct > 90.0 {
            CellColor::Ansi(1) // red
        } else if pct > 70.0 {
            CellColor::Ansi(3) // yellow
        } else {
            CellColor::Ansi(2) // green
        };

        let mut cells: Vec<Cell> = line.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg: color,
            bg: CellColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }).collect();
        cells.resize(self.cols, Cell::default());
        cells
    }
}

impl PanePlugin for SystemInfoPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::SystemInfo
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn resize(&mut self, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) {
        self.cols = (width_px / cell_w).floor().max(1.0) as usize;
        self.rows = (height_px / cell_h).floor().max(1.0) as usize;
        self.dirty = true;
    }

    fn render_data(&self) -> PanePluginRenderData {
        let mut lines: Vec<Vec<Cell>> = Vec::with_capacity(self.rows);

        // System section
        lines.push(self.make_section_header("SYSTEM"));
        lines.push(self.make_kv_line("Hostname", &self.info.hostname));
        lines.push(self.make_kv_line("macOS", &self.info.os_version));
        lines.push(self.make_kv_line("Load", &self.info.load_avg));
        lines.push(vec![Cell::default(); self.cols]);

        // CPU section
        lines.push(self.make_section_header("CPU"));
        lines.push(self.make_kv_line("Usage", &self.info.cpu_usage));
        lines.push(vec![Cell::default(); self.cols]);

        // Memory section
        lines.push(self.make_section_header("MEMORY"));
        lines.push(self.make_kv_line("RAM", &self.info.memory));
        // Extract percentage for bar
        if let Some(pct_str) = self.info.memory.split('(').last().and_then(|s| s.strip_suffix("%)")) {
            if let Ok(pct) = pct_str.parse::<f64>() {
                lines.push(self.make_bar("Usage", pct));
            }
        }
        lines.push(vec![Cell::default(); self.cols]);

        // Disk section
        lines.push(self.make_section_header("DISK"));
        lines.push(self.make_kv_line("Root (/)", &self.info.disk));
        lines.push(vec![Cell::default(); self.cols]);

        // Network section
        lines.push(self.make_section_header("NETWORK"));
        for iface in &self.info.network {
            lines.push(self.make_kv_line("Interface", iface));
        }
        if self.info.network.is_empty() {
            lines.push(self.make_kv_line("Status", "No active interfaces"));
        }

        // Pad to fill
        while lines.len() < self.rows {
            lines.push(vec![Cell::default(); self.cols]);
        }
        lines.truncate(self.rows);

        PanePluginRenderData::Terminal {
            lines,
            cursor: (usize::MAX, usize::MAX),
            watermark: None,
        }
    }

    fn visible_text(&self) -> String {
        format!(
            "Hostname: {}\nmacOS: {}\nCPU: {}\nMemory: {}\nDisk: {}\nLoad: {}\n",
            self.info.hostname,
            self.info.os_version,
            self.info.cpu_usage,
            self.info.memory,
            self.info.disk,
            self.info.load_avg,
        )
    }

    fn poll(&mut self) -> bool {
        if self.last_refresh.elapsed().as_millis() >= self.refresh_ms as u128 {
            self.last_refresh = Instant::now();
            self.refresh();
            return true;
        }
        false
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

fn run_cmd(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

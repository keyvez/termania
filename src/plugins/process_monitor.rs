use std::process::Command;
use std::time::Instant;

use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

struct ProcessEntry {
    pid: String,
    user: String,
    cpu: String,
    mem: String,
    command: String,
}

pub struct ProcessMonitorPlugin {
    title: String,
    entries: Vec<ProcessEntry>,
    cols: usize,
    rows: usize,
    dirty: bool,
    last_refresh: Instant,
    refresh_ms: u64,
    scroll_offset: usize,
}

impl ProcessMonitorPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Processes {}", index + 1));
        let refresh_ms = pane_config.and_then(|p| p.refresh_ms).unwrap_or(2000);

        let mut plugin = Self {
            title,
            entries: Vec::new(),
            cols: 80,
            rows: 24,
            dirty: true,
            last_refresh: Instant::now(),
            refresh_ms,
            scroll_offset: 0,
        };
        plugin.refresh();
        plugin
    }

    fn refresh(&mut self) {
        let output = Command::new("ps")
            .args(["aux", "--sort=-%cpu"])
            .output()
            .or_else(|_| {
                // macOS ps doesn't support --sort, use different approach
                Command::new("ps").args(["aux"]).output()
            });

        self.entries.clear();
        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut lines: Vec<&str> = text.lines().skip(1).collect(); // skip header
            // Sort by CPU% descending (field index 2)
            lines.sort_by(|a, b| {
                let cpu_a: f64 = a.split_whitespace().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let cpu_b: f64 = b.split_whitespace().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                cpu_b.partial_cmp(&cpu_a).unwrap_or(std::cmp::Ordering::Equal)
            });
            for line in lines.iter().take(200) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 11 {
                    self.entries.push(ProcessEntry {
                        user: parts[0].to_string(),
                        pid: parts[1].to_string(),
                        cpu: parts[2].to_string(),
                        mem: parts[3].to_string(),
                        command: parts[10..].join(" "),
                    });
                }
            }
        }
        self.dirty = true;
    }
}

impl PanePlugin for ProcessMonitorPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::ProcessMonitor
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

        // Header row
        let header = format!(
            " {:>6} {:>8} {:>5} {:>5}  {}",
            "PID", "USER", "%CPU", "%MEM", "COMMAND"
        );
        let header_cells: Vec<Cell> = header.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg: CellColor::Ansi(3),
            bg: CellColor::Default,
            bold: true,
            italic: false,
            underline: true,
            inverse: false,
        }).collect();
        lines.push(pad_line(header_cells, self.cols));

        // Process rows
        let visible = self.rows.saturating_sub(1);
        for i in 0..visible {
            let idx = self.scroll_offset + i;
            if idx < self.entries.len() {
                let e = &self.entries[idx];
                let line_str = format!(
                    " {:>6} {:>8} {:>5} {:>5}  {}",
                    e.pid, truncate_str(&e.user, 8), e.cpu, e.mem, e.command
                );
                let cpu_val: f64 = e.cpu.parse().unwrap_or(0.0);
                let fg = if cpu_val > 50.0 {
                    CellColor::Ansi(1) // red
                } else if cpu_val > 10.0 {
                    CellColor::Ansi(3) // yellow
                } else {
                    CellColor::Default
                };
                let cells: Vec<Cell> = line_str.chars().take(self.cols).map(|ch| Cell {
                    ch,
                    fg,
                    bg: CellColor::Default,
                    bold: false,
                    italic: false,
                    underline: false,
                    inverse: false,
                }).collect();
                lines.push(pad_line(cells, self.cols));
            } else {
                lines.push(vec![Cell::default(); self.cols]);
            }
        }

        PanePluginRenderData::Terminal {
            lines,
            cursor: (usize::MAX, usize::MAX),
            watermark: None,
        }
    }

    fn visible_text(&self) -> String {
        let mut text = String::from("PID USER %CPU %MEM COMMAND\n");
        for e in &self.entries {
            text.push_str(&format!("{} {} {} {} {}\n", e.pid, e.user, e.cpu, e.mem, e.command));
        }
        text
    }

    fn poll(&mut self) -> bool {
        if self.last_refresh.elapsed().as_millis() >= self.refresh_ms as u128 {
            self.last_refresh = Instant::now();
            self.refresh();
            return true;
        }
        false
    }

    fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true;
    }

    fn scroll_down(&mut self, lines: usize) {
        let max = self.entries.len().saturating_sub(self.rows.saturating_sub(1));
        self.scroll_offset = (self.scroll_offset + lines).min(max);
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}~", &s[..max - 1])
    }
}

fn pad_line(mut line: Vec<Cell>, cols: usize) -> Vec<Cell> {
    line.resize(cols, Cell::default());
    line
}

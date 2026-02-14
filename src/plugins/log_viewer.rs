use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Instant;

use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

pub struct LogViewerPlugin {
    title: String,
    file_path: String,
    lines: Vec<String>,
    cols: usize,
    rows: usize,
    dirty: bool,
    last_refresh: Instant,
    last_file_pos: u64,
    max_lines: usize,
    scroll_offset: usize,
}

impl LogViewerPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Logs {}", index + 1));
        let file_path = pane_config
            .and_then(|p| p.file.clone().or(p.path.clone()))
            .unwrap_or_else(|| "/var/log/system.log".to_string());
        let file_path = crate::config::expand_tilde(&file_path);

        let mut plugin = Self {
            title,
            file_path,
            lines: Vec::new(),
            cols: 80,
            rows: 24,
            dirty: true,
            last_refresh: Instant::now(),
            last_file_pos: 0,
            max_lines: 10000,
            scroll_offset: 0,
        };
        plugin.load_tail();
        plugin
    }

    fn load_tail(&mut self) {
        if let Ok(file) = File::open(&self.file_path) {
            let reader = BufReader::new(file);
            self.lines.clear();
            for line in reader.lines() {
                if let Ok(l) = line {
                    self.lines.push(l);
                    if self.lines.len() > self.max_lines {
                        self.lines.remove(0);
                    }
                }
            }
            self.last_file_pos = std::fs::metadata(&self.file_path)
                .map(|m| m.len())
                .unwrap_or(0);
            self.dirty = true;
        }
    }

    fn check_updates(&mut self) {
        let current_size = std::fs::metadata(&self.file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        if current_size < self.last_file_pos {
            // File was truncated, reload
            self.load_tail();
            return;
        }

        if current_size > self.last_file_pos {
            if let Ok(mut file) = File::open(&self.file_path) {
                if file.seek(SeekFrom::Start(self.last_file_pos)).is_ok() {
                    let reader = BufReader::new(file);
                    for line in reader.lines() {
                        if let Ok(l) = line {
                            self.lines.push(l);
                            if self.lines.len() > self.max_lines {
                                self.lines.remove(0);
                            }
                        }
                    }
                }
            }
            self.last_file_pos = current_size;
            self.dirty = true;
        }
    }

    fn colorize_line(line: &str, cols: usize) -> Vec<Cell> {
        let lower = line.to_lowercase();
        let (fg, bold) = if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") {
            (CellColor::Ansi(1), true) // red
        } else if lower.contains("warn") {
            (CellColor::Ansi(3), true) // yellow
        } else if lower.contains("info") {
            (CellColor::Ansi(2), false) // green
        } else if lower.contains("debug") || lower.contains("trace") {
            (CellColor::Ansi(8), false) // dark gray
        } else {
            (CellColor::Default, false)
        };

        let mut cells: Vec<Cell> = line.chars().take(cols).map(|ch| Cell {
            ch,
            fg,
            bg: CellColor::Default,
            bold,
            italic: false,
            underline: false,
            inverse: false,
        }).collect();
        cells.resize(cols, Cell::default());
        cells
    }
}

impl PanePlugin for LogViewerPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::LogViewer
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
        let mut rendered: Vec<Vec<Cell>> = Vec::with_capacity(self.rows);

        // Show file path header
        let header = format!(" {} ({} lines)", self.file_path, self.lines.len());
        let header_cells: Vec<Cell> = header.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg: CellColor::Ansi(6), // cyan
            bg: CellColor::Default,
            bold: true,
            italic: false,
            underline: true,
            inverse: false,
        }).collect();
        rendered.push(pad_line(header_cells, self.cols));

        let visible = self.rows.saturating_sub(1);
        let total = self.lines.len();
        // Show the tail of the log, adjusted by scroll_offset
        let end = total.saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(visible);

        for i in start..end {
            rendered.push(Self::colorize_line(&self.lines[i], self.cols));
        }

        // Pad remaining rows
        while rendered.len() < self.rows {
            rendered.push(vec![Cell::default(); self.cols]);
        }

        PanePluginRenderData::Terminal {
            lines: rendered,
            cursor: (usize::MAX, usize::MAX),
            watermark: None,
        }
    }

    fn visible_text(&self) -> String {
        let start = self.lines.len().saturating_sub(self.rows);
        self.lines[start..].join("\n")
    }

    fn poll(&mut self) -> bool {
        if self.last_refresh.elapsed().as_millis() >= 500 {
            self.last_refresh = Instant::now();
            self.check_updates();
            return self.dirty;
        }
        false
    }

    fn scroll_up(&mut self, lines: usize) {
        let max = self.lines.len().saturating_sub(self.rows);
        self.scroll_offset = (self.scroll_offset + lines).min(max);
        self.dirty = true;
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

fn pad_line(mut line: Vec<Cell>, cols: usize) -> Vec<Cell> {
    line.resize(cols, Cell::default());
    line
}

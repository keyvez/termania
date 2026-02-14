use std::process::Command;
use std::time::Instant;

use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

pub struct GitStatusPlugin {
    title: String,
    repo_path: String,
    cols: usize,
    rows: usize,
    dirty: bool,
    last_refresh: Instant,
    scroll_offset: usize,
    branch: String,
    status_lines: Vec<StatusLine>,
    log_lines: Vec<String>,
    view: GitView,
}

#[derive(Clone, Copy, PartialEq)]
enum GitView {
    Status,
    Log,
    Diff,
}

struct StatusLine {
    status: String,
    file: String,
}

impl GitStatusPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Git {}", index + 1));
        let repo_path = pane_config
            .and_then(|p| p.repo.clone().or(p.path.clone()))
            .unwrap_or_else(|| ".".to_string());
        let repo_path = crate::config::expand_tilde(&repo_path);

        let mut plugin = Self {
            title,
            repo_path,
            cols: 80,
            rows: 24,
            dirty: true,
            last_refresh: Instant::now(),
            scroll_offset: 0,
            branch: String::new(),
            status_lines: Vec::new(),
            log_lines: Vec::new(),
            view: GitView::Status,
        };
        plugin.refresh();
        plugin
    }

    fn refresh(&mut self) {
        self.branch = self.git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"]);
        self.refresh_status();
        self.refresh_log();
        self.dirty = true;
    }

    fn refresh_status(&mut self) {
        let output = self.git_cmd(&["status", "--porcelain"]);
        self.status_lines = output
            .lines()
            .map(|line| {
                let (status, file) = if line.len() > 3 {
                    (line[..2].to_string(), line[3..].to_string())
                } else {
                    (line.to_string(), String::new())
                };
                StatusLine { status, file }
            })
            .collect();
    }

    fn refresh_log(&mut self) {
        let output = self.git_cmd(&["log", "--oneline", "--graph", "-20"]);
        self.log_lines = output.lines().map(|l| l.to_string()).collect();
    }

    fn git_cmd(&self, args: &[&str]) -> String {
        Command::new("git")
            .args(["-C", &self.repo_path])
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn get_diff_lines(&self) -> Vec<String> {
        let output = self.git_cmd(&["diff", "--stat"]);
        output.lines().map(|l| l.to_string()).collect()
    }

    fn make_header(&self, text: &str) -> Vec<Cell> {
        let mut cells: Vec<Cell> = text.chars().take(self.cols).map(|ch| Cell {
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

    fn colorize_status_line(&self, sl: &StatusLine) -> Vec<Cell> {
        let line = format!(" {} {}", sl.status, sl.file);
        let fg = match sl.status.trim() {
            "M" | "MM" => CellColor::Ansi(3), // yellow - modified
            "A" | "AM" => CellColor::Ansi(2), // green - added
            "D" => CellColor::Ansi(1),         // red - deleted
            "??" => CellColor::Ansi(8),        // gray - untracked
            "R" => CellColor::Ansi(4),         // blue - renamed
            _ => CellColor::Default,
        };
        let mut cells: Vec<Cell> = line.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg,
            bg: CellColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }).collect();
        cells.resize(self.cols, Cell::default());
        cells
    }

    fn colorize_log_line(&self, line: &str) -> Vec<Cell> {
        let mut cells: Vec<Cell> = Vec::new();
        let mut chars = line.chars().peekable();

        // Graph characters are typically *, |, /, \
        let mut in_graph = true;
        while let Some(ch) = chars.next() {
            if in_graph && (ch == '*' || ch == '|' || ch == '/' || ch == '\\' || ch == ' ') {
                let fg = if ch == '*' { CellColor::Ansi(1) } else { CellColor::Ansi(3) };
                cells.push(Cell {
                    ch,
                    fg,
                    bg: CellColor::Default,
                    bold: ch == '*',
                    italic: false,
                    underline: false,
                    inverse: false,
                });
            } else {
                in_graph = false;
                // First 7 chars after graph are hash
                let fg = if cells.len() < 15 {
                    CellColor::Ansi(3) // yellow for hash
                } else {
                    CellColor::Default
                };
                cells.push(Cell {
                    ch,
                    fg,
                    bg: CellColor::Default,
                    bold: false,
                    italic: false,
                    underline: false,
                    inverse: false,
                });
            }
            if cells.len() >= self.cols {
                break;
            }
        }
        cells.resize(self.cols, Cell::default());
        cells
    }

    fn colorize_diff_line(&self, line: &str) -> Vec<Cell> {
        let fg = if line.starts_with('+') {
            CellColor::Ansi(2) // green
        } else if line.starts_with('-') {
            CellColor::Ansi(1) // red
        } else if line.starts_with('@') {
            CellColor::Ansi(6) // cyan
        } else {
            CellColor::Default
        };
        let mut cells: Vec<Cell> = line.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg,
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

impl PanePlugin for GitStatusPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::GitStatus
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

    fn handle_key(&mut self, text: &str) {
        match text {
            "s" | "1" => {
                self.view = GitView::Status;
                self.scroll_offset = 0;
                self.dirty = true;
            }
            "l" | "2" => {
                self.view = GitView::Log;
                self.scroll_offset = 0;
                self.dirty = true;
            }
            "d" | "3" => {
                self.view = GitView::Diff;
                self.scroll_offset = 0;
                self.dirty = true;
            }
            "r" => {
                self.refresh();
            }
            _ => {}
        }
    }

    fn render_data(&self) -> PanePluginRenderData {
        let mut lines: Vec<Vec<Cell>> = Vec::with_capacity(self.rows);

        // Branch header
        let branch_line = format!(
            " \u{e0a0} {} | [s]tatus [l]og [d]iff [r]efresh",
            self.branch
        );
        lines.push(self.make_header(&branch_line));

        // View-specific content
        let visible = self.rows.saturating_sub(1);
        match self.view {
            GitView::Status => {
                if self.status_lines.is_empty() {
                    let clean = " Working tree clean";
                    let mut cells: Vec<Cell> = clean.chars().map(|ch| Cell {
                        ch,
                        fg: CellColor::Ansi(2),
                        bg: CellColor::Default,
                        bold: false,
                        italic: false,
                        underline: false,
                        inverse: false,
                    }).collect();
                    cells.resize(self.cols, Cell::default());
                    lines.push(cells);
                } else {
                    for i in 0..visible {
                        let idx = self.scroll_offset + i;
                        if idx < self.status_lines.len() {
                            lines.push(self.colorize_status_line(&self.status_lines[idx]));
                        }
                    }
                }
            }
            GitView::Log => {
                for i in 0..visible {
                    let idx = self.scroll_offset + i;
                    if idx < self.log_lines.len() {
                        lines.push(self.colorize_log_line(&self.log_lines[idx]));
                    }
                }
            }
            GitView::Diff => {
                let diff_lines = self.get_diff_lines();
                for i in 0..visible {
                    let idx = self.scroll_offset + i;
                    if idx < diff_lines.len() {
                        lines.push(self.colorize_diff_line(&diff_lines[idx]));
                    }
                }
            }
        }

        // Pad
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
        let mut text = format!("Branch: {}\n", self.branch);
        for sl in &self.status_lines {
            text.push_str(&format!("{} {}\n", sl.status, sl.file));
        }
        text
    }

    fn poll(&mut self) -> bool {
        if self.last_refresh.elapsed().as_secs() >= 5 {
            self.last_refresh = Instant::now();
            self.refresh();
            return true;
        }
        false
    }

    fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.dirty = true;
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll_offset += n;
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

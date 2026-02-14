use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

pub struct MarkdownPreviewPlugin {
    title: String,
    content: String,
    rendered_lines: Vec<RenderedLine>,
    cols: usize,
    rows: usize,
    dirty: bool,
    scroll_offset: usize,
    file_path: Option<String>,
}

struct RenderedLine {
    cells: Vec<Cell>,
}

impl MarkdownPreviewPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Markdown {}", index + 1));

        let file_path = pane_config.and_then(|p| p.file.clone().or(p.path.clone()));
        let content = if let Some(ref path) = file_path {
            let path = crate::config::expand_tilde(path);
            std::fs::read_to_string(&path).unwrap_or_else(|_| format!("# Error\nCould not read: {}", path))
        } else {
            pane_config
                .and_then(|p| p.content.clone())
                .unwrap_or_else(|| "# Markdown Preview\n\nNo content loaded.".to_string())
        };

        let mut plugin = Self {
            title,
            content,
            rendered_lines: Vec::new(),
            cols: 80,
            rows: 24,
            dirty: true,
            scroll_offset: 0,
            file_path,
        };
        plugin.render_markdown();
        plugin
    }

    fn render_markdown(&mut self) {
        self.rendered_lines.clear();

        let mut in_code_block = false;

        for line in self.content.lines() {
            if line.starts_with("```") {
                in_code_block = !in_code_block;
                let cells = self.make_cells(line, CellColor::Ansi(8), false, false);
                self.rendered_lines.push(RenderedLine { cells });
                continue;
            }

            if in_code_block {
                let cells = self.make_cells(
                    &format!("  {}", line),
                    CellColor::Ansi(6), // cyan
                    false,
                    false,
                );
                self.rendered_lines.push(RenderedLine { cells });
                continue;
            }

            if line.starts_with("# ") {
                let text = &line[2..];
                let cells = self.make_cells(text, CellColor::Ansi(5), true, true); // magenta, bold, underline
                // Add empty line before heading
                self.rendered_lines.push(RenderedLine { cells: vec![Cell::default(); self.cols] });
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.starts_with("## ") {
                let text = &line[3..];
                let cells = self.make_cells(text, CellColor::Ansi(4), true, true); // blue, bold, underline
                self.rendered_lines.push(RenderedLine { cells: vec![Cell::default(); self.cols] });
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.starts_with("### ") {
                let text = &line[4..];
                let cells = self.make_cells(text, CellColor::Ansi(2), true, false); // green, bold
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.starts_with("- ") || line.starts_with("* ") {
                let text = format!("  {} {}", '\u{2022}', &line[2..]);
                let cells = self.make_cells(&text, CellColor::Default, false, false);
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.starts_with("> ") {
                let text = format!("  | {}", &line[2..]);
                let cells = self.make_cells(&text, CellColor::Ansi(3), false, true); // yellow, italic via underline
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.starts_with("---") || line.starts_with("***") || line.starts_with("___") {
                let divider: String = "\u{2500}".repeat(self.cols);
                let cells = self.make_cells(&divider, CellColor::Ansi(8), false, false);
                self.rendered_lines.push(RenderedLine { cells });
            } else if line.trim().is_empty() {
                self.rendered_lines.push(RenderedLine { cells: vec![Cell::default(); self.cols] });
            } else {
                // Render inline formatting
                let cells = self.render_inline(line);
                self.rendered_lines.push(RenderedLine { cells });
            }
        }
        self.dirty = true;
    }

    fn make_cells(&self, text: &str, fg: CellColor, bold: bool, underline: bool) -> Vec<Cell> {
        let mut cells: Vec<Cell> = text.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg,
            bg: CellColor::Default,
            bold,
            italic: false,
            underline,
            inverse: false,
        }).collect();
        cells.resize(self.cols, Cell::default());
        cells
    }

    fn render_inline(&self, line: &str) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(self.cols);
        let mut chars = line.chars().peekable();
        let mut bold = false;
        let mut italic = false;
        let mut code = false;

        while let Some(ch) = chars.next() {
            if ch == '`' && !code {
                code = true;
                continue;
            } else if ch == '`' && code {
                code = false;
                continue;
            }

            if ch == '*' && !code {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    bold = !bold;
                } else {
                    italic = !italic;
                }
                continue;
            }

            let fg = if code {
                CellColor::Ansi(6)
            } else {
                CellColor::Default
            };

            cells.push(Cell {
                ch,
                fg,
                bg: CellColor::Default,
                bold,
                italic,
                underline: false,
                inverse: false,
            });

            if cells.len() >= self.cols {
                break;
            }
        }

        cells.resize(self.cols, Cell::default());
        cells
    }
}

impl PanePlugin for MarkdownPreviewPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::MarkdownPreview
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn set_content(&mut self, content: &str) -> bool {
        self.content = content.to_string();
        self.render_markdown();
        true
    }

    fn resize(&mut self, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) {
        self.cols = (width_px / cell_w).floor().max(1.0) as usize;
        self.rows = (height_px / cell_h).floor().max(1.0) as usize;
        self.render_markdown();
    }

    fn render_data(&self) -> PanePluginRenderData {
        let mut lines: Vec<Vec<Cell>> = Vec::with_capacity(self.rows);

        let start = self.scroll_offset;
        for i in 0..self.rows {
            let idx = start + i;
            if idx < self.rendered_lines.len() {
                lines.push(self.rendered_lines[idx].cells.clone());
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
        self.content.clone()
    }

    fn scroll_up(&mut self, lines: usize) {
        let max = self.rendered_lines.len().saturating_sub(self.rows);
        self.scroll_offset = (self.scroll_offset + lines).min(max);
        self.dirty = true;
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true;
    }

    fn poll(&mut self) -> bool {
        // Reload file if watching a file
        if let Some(ref path) = self.file_path {
            let path = crate::config::expand_tilde(path);
            if let Ok(new_content) = std::fs::read_to_string(&path) {
                if new_content != self.content {
                    self.content = new_content;
                    self.render_markdown();
                    return true;
                }
            }
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

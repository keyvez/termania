use std::time::Instant;

use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::terminal::{Cell, CellColor};

/// A directory entry in the file browser
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
    depth: usize,
    expanded: bool,
}

pub struct FileBrowserPlugin {
    title: String,
    root_path: String,
    entries: Vec<Entry>,
    selected: usize,
    cols: usize,
    rows: usize,
    dirty: bool,
    scroll_offset: usize,
    last_refresh: Instant,
}

impl FileBrowserPlugin {
    pub fn new(index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Files {}", index + 1));
        let root_path = pane_config
            .and_then(|p| p.path.clone())
            .unwrap_or_else(|| ".".to_string());
        let root_path = crate::config::expand_tilde(&root_path);

        let mut plugin = Self {
            title,
            root_path,
            entries: Vec::new(),
            selected: 0,
            cols: 80,
            rows: 24,
            dirty: true,
            scroll_offset: 0,
            last_refresh: Instant::now(),
        };
        plugin.refresh_entries();
        plugin
    }

    fn refresh_entries(&mut self) {
        self.entries.clear();
        self.scan_dir(&self.root_path.clone(), 0);
        self.dirty = true;
    }

    fn scan_dir(&mut self, path: &str, depth: usize) {
        let mut items: Vec<(String, bool, u64)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue; // Skip hidden files
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                items.push((name, is_dir, size));
            }
        }
        items.sort_by(|a, b| {
            b.1.cmp(&a.1) // Dirs first
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        for (name, is_dir, size) in items {
            self.entries.push(Entry {
                name,
                is_dir,
                size,
                depth,
                expanded: false,
            });
        }
    }

    fn toggle_dir(&mut self) {
        if self.selected >= self.entries.len() {
            return;
        }
        if !self.entries[self.selected].is_dir {
            return;
        }

        let entry = &self.entries[self.selected];
        let depth = entry.depth;

        if entry.expanded {
            // Collapse: remove children
            let mut i = self.selected + 1;
            while i < self.entries.len() && self.entries[i].depth > depth {
                i += 1;
            }
            self.entries[self.selected].expanded = false;
            if i > self.selected + 1 {
                self.entries.drain((self.selected + 1)..i);
            }
        } else {
            // Expand: insert children
            self.entries[self.selected].expanded = true;
            let path = self.build_path(self.selected);
            let insert_at = self.selected + 1;
            let mut children = Vec::new();

            if let Ok(entries) = std::fs::read_dir(&path) {
                let mut items: Vec<(String, bool, u64)> = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    items.push((name, is_dir, size));
                }
                items.sort_by(|a, b| {
                    b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
                });
                for (name, is_dir, size) in items {
                    children.push(Entry {
                        name,
                        is_dir,
                        size,
                        depth: depth + 1,
                        expanded: false,
                    });
                }
            }
            // Insert children after current entry
            for (i, child) in children.into_iter().enumerate() {
                self.entries.insert(insert_at + i, child);
            }
        }
        self.dirty = true;
    }

    fn build_path(&self, idx: usize) -> String {
        let mut parts = vec![self.entries[idx].name.clone()];
        let mut depth = self.entries[idx].depth;
        let mut i = idx;
        while depth > 0 && i > 0 {
            i -= 1;
            if self.entries[i].depth < depth && self.entries[i].is_dir {
                parts.push(self.entries[i].name.clone());
                depth = self.entries[i].depth;
            }
        }
        parts.reverse();
        format!("{}/{}", self.root_path, parts.join("/"))
    }

    fn format_size(size: u64) -> String {
        if size < 1024 {
            format!("{}B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1}K", size as f64 / 1024.0)
        } else if size < 1024 * 1024 * 1024 {
            format!("{:.1}M", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1}G", size as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    fn render_line(&self, entry: &Entry, is_selected: bool) -> Vec<Cell> {
        let indent = "  ".repeat(entry.depth);
        let icon = if entry.is_dir {
            if entry.expanded { "v " } else { "> " }
        } else {
            "  "
        };
        let size_str = if entry.is_dir {
            String::new()
        } else {
            Self::format_size(entry.size)
        };

        let name_part = format!("{}{}{}", indent, icon, entry.name);
        let padding = if self.cols > name_part.len() + size_str.len() {
            self.cols - name_part.len() - size_str.len()
        } else {
            1
        };
        let line = format!("{}{:>width$}{}", name_part, "", size_str, width = padding);

        let fg = if entry.is_dir {
            CellColor::Ansi(4) // blue
        } else {
            CellColor::Default
        };
        let bg = if is_selected {
            CellColor::Ansi(8) // bright black / dark gray
        } else {
            CellColor::Default
        };

        line.chars()
            .take(self.cols)
            .map(|ch| Cell {
                ch,
                fg,
                bg,
                bold: entry.is_dir,
                italic: false,
                underline: false,
                inverse: false,
            })
            .collect()
    }
}

impl PanePlugin for FileBrowserPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::FileBrowser
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn resize(&mut self, width_px: f32, _height_px: f32, cell_w: f32, cell_h: f32) {
        self.cols = (width_px / cell_w).floor().max(1.0) as usize;
        self.rows = (_height_px / cell_h).floor().max(1.0) as usize;
        self.dirty = true;
    }

    fn handle_key(&mut self, text: &str) {
        match text {
            "j" | "\x1b[B" => {
                if self.selected + 1 < self.entries.len() {
                    self.selected += 1;
                    // Auto-scroll
                    if self.selected >= self.scroll_offset + self.rows.saturating_sub(1) {
                        self.scroll_offset = self.selected.saturating_sub(self.rows.saturating_sub(2));
                    }
                    self.dirty = true;
                }
            }
            "k" | "\x1b[A" => {
                if self.selected > 0 {
                    self.selected -= 1;
                    if self.selected < self.scroll_offset {
                        self.scroll_offset = self.selected;
                    }
                    self.dirty = true;
                }
            }
            "\r" | "l" | "\x1b[C" => {
                self.toggle_dir();
            }
            "h" | "\x1b[D" => {
                // Collapse or go to parent
                if self.selected < self.entries.len() && self.entries[self.selected].is_dir && self.entries[self.selected].expanded {
                    self.toggle_dir();
                }
            }
            "r" => {
                self.refresh_entries();
            }
            _ => {}
        }
    }

    fn render_data(&self) -> PanePluginRenderData {
        let mut lines: Vec<Vec<Cell>> = Vec::with_capacity(self.rows);

        // Header
        let header = format!(" {} ", self.root_path);
        let header_line: Vec<Cell> = header.chars().take(self.cols).map(|ch| Cell {
            ch,
            fg: CellColor::Ansi(3), // yellow
            bg: CellColor::Default,
            bold: true,
            italic: false,
            underline: true,
            inverse: false,
        }).collect();
        lines.push(pad_line(header_line, self.cols));

        // Entries
        let visible_rows = self.rows.saturating_sub(1);
        for i in 0..visible_rows {
            let idx = self.scroll_offset + i;
            if idx < self.entries.len() {
                let line = self.render_line(&self.entries[idx], idx == self.selected);
                lines.push(pad_line(line, self.cols));
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
        let mut text = format!("{}\n", self.root_path);
        for entry in &self.entries {
            let indent = "  ".repeat(entry.depth);
            let icon = if entry.is_dir { "/ " } else { "  " };
            text.push_str(&format!("{}{}{}\n", indent, icon, entry.name));
        }
        text
    }

    fn poll(&mut self) -> bool {
        if self.last_refresh.elapsed().as_secs() >= 5 {
            self.last_refresh = Instant::now();
            self.refresh_entries();
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

fn pad_line(mut line: Vec<Cell>, cols: usize) -> Vec<Cell> {
    line.resize(cols, Cell::default());
    line
}

use crate::config::{Config, PaneConfig, expand_tilde};
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};
use crate::pty::Pty;
use crate::terminal::Terminal;

/// Terminal plugin — wraps the existing Terminal + PTY as a PanePlugin
pub struct TerminalPlugin {
    terminal: Terminal,
}

impl TerminalPlugin {
    /// Create a new terminal plugin from a pane config
    pub fn new(index: usize, pane_config: Option<&PaneConfig>, _config: &Config) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| format!("Pane {}", index + 1));

        let cols = 80;
        let rows = 24;
        let mut terminal = Terminal::new(cols, rows, &title);

        // Queue initial commands
        if let Some(cmds) = pane_config.and_then(|p| p.initial_commands.as_ref()) {
            terminal.set_initial_commands(cmds.clone());
        }

        // Spawn PTY
        let shell = pane_config.and_then(|p| p.command.clone());
        let cwd = pane_config.and_then(|p| p.cwd.as_deref().map(expand_tilde));

        match Pty::spawn(cols as u16, rows as u16, shell.as_deref(), cwd.as_deref()) {
            Ok(pty) => {
                terminal.set_pty(pty);
            }
            Err(e) => {
                log::error!("Failed to spawn PTY for pane {}: {}", index, e);
            }
        }

        Self { terminal }
    }

    /// Create a bare terminal plugin (for Cmd+N dynamic spawn)
    pub fn new_bare(index: usize) -> Self {
        let title = format!("Pane {}", index + 1);
        let mut terminal = Terminal::new(80, 24, &title);

        match Pty::spawn(80, 24, None, None) {
            Ok(pty) => terminal.set_pty(pty),
            Err(e) => log::error!("Failed to spawn PTY: {}", e),
        }

        Self { terminal }
    }

    /// Get access to the inner Terminal (needed for rendering cell data directly)
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }
}

impl PanePlugin for TerminalPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::Terminal
    }

    fn title(&self) -> &str {
        self.terminal.title()
    }

    fn set_title(&mut self, title: String) {
        self.terminal.set_title(title);
    }

    fn resize(&mut self, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) {
        let cols = (width_px / cell_w).floor().max(1.0) as usize;
        let rows = (height_px / cell_h).floor().max(1.0) as usize;
        log::info!("terminal resize: width_px={:.1}, cell_w={:.1}, cols={}, height_px={:.1}, cell_h={:.1}, rows={}",
            width_px, cell_w, cols, height_px, cell_h, rows);
        self.terminal.resize(cols, rows);
    }

    fn render_data(&self) -> PanePluginRenderData {
        let is_scrolled = self.terminal.scroll_offset() > 0;
        PanePluginRenderData::Terminal {
            lines: self.terminal.scrolled_lines(),
            cursor: if is_scrolled {
                // Hide cursor when scrolled back (show no cursor position)
                (usize::MAX, usize::MAX)
            } else {
                self.terminal.cursor_position()
            },
            watermark: None, // watermark is handled separately from config
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        self.terminal.scroll_view_up(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.terminal.scroll_view_down(lines);
    }

    fn visible_text(&self) -> String {
        self.terminal.visible_text()
    }

    fn write_input(&mut self, data: &[u8]) {
        self.terminal.write_input(data);
    }

    fn poll(&mut self) -> bool {
        let was_dirty = self.terminal.is_dirty();
        self.terminal.poll();
        // Return true if content changed during this poll
        self.terminal.is_dirty() && !was_dirty
    }

    fn has_error(&self) -> bool {
        self.terminal.has_error()
    }

    fn is_exited(&self) -> bool {
        self.terminal.is_exited()
    }

    fn is_dirty(&self) -> bool {
        self.terminal.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.terminal.clear_dirty();
    }

    fn is_native_view(&self) -> bool {
        false
    }
}
